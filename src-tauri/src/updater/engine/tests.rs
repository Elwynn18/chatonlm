//! Essais de la machine d'états contre un faux serveur de mises à jour
//! (HTTP local) et le vrai greffon, construit dans une app simulée
//! (`tauri::test`). Paquets et signatures : ceux de `staging::tests` (clé
//! d'essai, version 9.9.9 ; AppImage par défaut) ; app en 0.1.0.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::testkit::*;
use super::*;
use crate::updater::UnsupportedReason;
use crate::updater::staging::tests::{
    APPIMAGE_RESIGNED, APPIMAGE_SIGNATURE, DEB_SIGNATURE, TEST_PUBKEY, test_appimage, test_deb,
};

/// Contenu de l'AppImage lancée (0.1.0).
const OLD_APPIMAGE: &[u8] = b"ChatonLM 0.1.0";

/// Dossier d'essai : AppImage factice (0.1.0) et dossier des mises à jour.
struct Rig {
    _dir: tempfile::TempDir,
    appimage: PathBuf,
    staging: PathBuf,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let apps = dir.path().join("Apps");
    std::fs::create_dir(&apps).unwrap();
    let appimage = apps.join("ChatonLM.AppImage");
    std::fs::write(&appimage, OLD_APPIMAGE).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o755)).unwrap();
    let staging = dir.path().join("data").join("updates");
    Rig {
        _dir: dir,
        appimage,
        staging,
    }
}

fn engine(server: &FakeServer, rig: &Rig, auto_download: bool) -> Arc<Engine> {
    Arc::new(Engine::new(Config {
        auto_download,
        ..config_for(server, rig)
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_is_downloaded_kept_and_found_again_after_a_relaunch() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    assert_eq!(engine.status().state, UpdateState::Idle);
    assert_eq!(engine.status().installer, Some(UpdateInstaller::AppImage));

    let checked = engine.check(Trigger::Scheduled).await;
    assert!(checked.checked_at.is_some());
    let status = wait_for(&engine, "prête", ready("9.9.9")).await;
    assert_eq!(
        status.state,
        UpdateState::Ready {
            version: "9.9.9".into(),
            notes: Some("Faster answers.".into())
        }
    );
    assert_eq!(server.count("/package"), 1);
    assert_eq!(
        files(&rig.staging),
        ["chatonlm-9.9.9.update", "update.json"]
    );
    assert_eq!(
        std::fs::read(rig.staging.join("chatonlm-9.9.9.update")).unwrap(),
        test_appimage()
    );

    // Relance de ChatonLM sans installation : prête tout de suite (signature
    // revérifiée), sans nouveau téléchargement, y compris après la
    // vérification suivante.
    let relaunched = self::engine(&server, &rig, true);
    relaunched.restore().await;
    assert!(ready("9.9.9")(&relaunched.status().state));
    let checked = relaunched.check(Trigger::Scheduled).await;
    assert!(ready("9.9.9")(&checked.state));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.count("/package"), 1);
    assert_eq!(server.count("/latest.json"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_states_follow_the_official_order() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    // Réponses lentes : chaque état dure assez pour être lu (canal watch :
    // seul le dernier état compte).
    server.served.lock().delay = Duration::from_millis(300);
    server.served.lock().package_delay = Duration::from_millis(300);
    let engine = engine(&server, &rig, true);
    let mut receiver = engine.subscribe();
    let name = |state: &UpdateState| match state {
        UpdateState::Idle => "idle",
        UpdateState::Checking => "checking",
        UpdateState::Available { .. } => "available",
        UpdateState::Downloading { .. } => "downloading",
        UpdateState::Ready { .. } => "ready",
        _ => "other",
    };
    // État de départ lu avant la vérification, puis chaque changement.
    let seen = Arc::new(Mutex::new(vec![name(&receiver.borrow_and_update().state)]));
    let log = Arc::clone(&seen);
    let watcher = tokio::spawn(async move {
        while receiver.changed().await.is_ok() {
            let state = name(&receiver.borrow_and_update().state);
            if log.lock().last() != Some(&state) {
                log.lock().push(state);
            }
            if state == "ready" {
                break;
            }
        }
    });
    engine.check(Trigger::Requested).await;
    tokio::time::timeout(WAIT, watcher).await.unwrap().unwrap();
    let seen = seen.lock().clone();
    // « available » est aussitôt remplacé par le téléchargement qui commence.
    let seen: Vec<_> = seen
        .into_iter()
        .filter(|state| *state != "available")
        .collect();
    assert_eq!(seen, ["idle", "checking", "downloading", "ready"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_auto_download_an_update_is_only_reported() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, false);
    let status = engine.check(Trigger::Scheduled).await;
    assert_eq!(
        status.state,
        UpdateState::Available {
            version: "9.9.9".into(),
            notes: Some("Faster answers.".into()),
            date: Some("2026-10-07T12:00:00Z".into()),
        }
    );
    assert!(!status.auto_download);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.count("/package"), 0);
    assert!(matches!(
        engine.status().state,
        UpdateState::Available { .. }
    ));

    // « Download » : téléchargée sur demande.
    let started = engine.download().unwrap();
    assert!(matches!(started.state, UpdateState::Downloading { .. }));
    wait_for(&engine, "prête", ready("9.9.9")).await;
    assert_eq!(server.count("/package"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_auto_download_on_downloads_a_reported_update() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, false);
    engine.check(Trigger::Requested).await;
    let status = engine.set_auto_download(true);
    assert!(status.auto_download);
    wait_for(&engine, "prête", ready("9.9.9")).await;
    // Coupé ensuite : rien ne change de ce qui est prêt.
    assert!(!engine.set_auto_download(false).auto_download);
    assert!(ready("9.9.9")(&engine.status().state));
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_to_download_without_an_announced_update() {
    let server = FakeServer::start().await;
    let rig = rig();
    let engine = engine(&server, &rig, true);
    assert!(engine.download().is_err());
    let status = engine.check(Trigger::Requested).await;
    assert_eq!(status.state, UpdateState::UpToDate);
    assert!(engine.download().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_signature_keeps_nothing() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let mut tampered = test_appimage();
    tampered[4242] ^= 0x80;
    server.served.lock().package = tampered;
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    let status = wait_for(&engine, "erreur", |state| {
        matches!(state, UpdateState::Error { .. })
    })
    .await;
    let UpdateState::Error { code, detail } = status.state else {
        unreachable!()
    };
    assert_eq!(code, UpdateErrorCode::Signature, "{detail}");
    assert!(files(&rig.staging).is_empty());
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), OLD_APPIMAGE);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_announce_for_another_version_than_signed_is_refused() {
    // Signature de la 9.9.9 présentée comme 9.9.10 (rejeu d'un vieux paquet
    // sous un numéro plus grand).
    let server = FakeServer::start().await;
    server.announce("9.9.10");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    let status = wait_for(&engine, "erreur", |state| {
        matches!(state, UpdateState::Error { .. })
    })
    .await;
    assert!(matches!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Signature,
            ..
        }
    ));
    assert!(files(&rig.staging).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_newer_means_up_to_date_and_drops_an_old_download() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;

    // Version retirée du serveur (204) : la mise à jour gardée est dépassée.
    server.served.lock().manifest = None;
    let status = engine.check(Trigger::Requested).await;
    assert_eq!(status.state, UpdateState::UpToDate);
    assert!(files(&rig.staging).is_empty());

    // Version plus ancienne que l'app annoncée : à jour aussi.
    server.announce("0.0.9");
    let status = engine.check(Trigger::Requested).await;
    assert_eq!(status.state, UpdateState::UpToDate);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_check_is_an_error_but_a_ready_update_stays_ready() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    server.served.lock().status = 500;
    let rig = rig();
    let engine = engine(&server, &rig, true);
    let status = engine.check(Trigger::Scheduled).await;
    // Réponse non 2xx ignorée par le greffon (ReleaseNotFound) : dite dans
    // la langue de l'interface, pas par son message anglais.
    assert_eq!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Check,
            detail: t!("updates.errors.noRelease"),
        }
    );
    assert_eq!(status.checked_at, None);

    server.served.lock().status = 200;
    engine.check(Trigger::Requested).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;
    server.served.lock().status = 500;
    // Prête : elle le reste, l'échec est noté à côté, puis effacé par la
    // vérification réussie suivante.
    for trigger in [Trigger::Scheduled, Trigger::Requested] {
        let status = engine.check(trigger).await;
        assert!(ready("9.9.9")(&status.state));
        let error = status.last_error.expect("échec noté");
        assert_eq!(error.code, UpdateErrorCode::Check);
        assert_eq!(error.detail, t!("updates.errors.noRelease"));
    }
    server.served.lock().status = 200;
    let status = engine.check(Trigger::Scheduled).await;
    assert!(ready("9.9.9")(&status.state));
    assert_eq!(status.last_error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_release_is_said_in_the_interface_language() {
    // Dépôt GitHub privé ou Release sans latest.json : 404.
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    server.served.lock().status = 404;
    let rig = rig();
    let engine = engine(&server, &rig, true);
    let status = engine.check(Trigger::Requested).await;
    assert_eq!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Check,
            detail: t!("updates.errors.noRelease"),
        }
    );
    let UpdateState::Error { detail, .. } = status.state else {
        unreachable!()
    };
    assert!(!detail.contains("Could not fetch"), "{detail}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_is_said_without_its_address() {
    // Port libéré aussitôt réservé : connexion refusée.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let endpoint = format!("http://127.0.0.1:{port}/latest.json");
    let rig = rig();
    let engine = Arc::new(Engine::new(Config {
        updater: Some(plugin_updater(&endpoint, &rig.appimage)),
        support: Support::Installable(UpdateInstaller::AppImage),
        platform: Platform {
            appimage: Some(rig.appimage.clone()),
            pkexec: None,
        },
        staging: rig.staging.clone(),
        pubkey: TEST_PUBKEY.into(),
        require_signed_version: true,
        current: Version::new(0, 1, 0),
        auto_download: true,
    }));
    let status = engine.check(Trigger::Requested).await;
    let UpdateState::Error {
        code: UpdateErrorCode::Check,
        detail,
    } = status.state
    else {
        panic!("{:?}", status.state)
    };
    let title = t!("updates.errors.unreachable", error = "");
    let title = title.trim_end();
    assert!(detail.starts_with(title), "{detail}");
    // Les causes de reqwest suivent le titre, sans l'adresse.
    assert!(detail.len() > title.len() + 2, "{detail}");
    assert!(!detail.contains(&endpoint), "{detail}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_download_says_the_http_status() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    // Paquet annoncé à une adresse que le serveur ne connaît pas (404).
    if let Some(manifest) = server.served.lock().manifest.as_mut() {
        manifest["platforms"]["linux-x86_64"]["url"] = format!("{}/missing", server.base).into();
    }
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    let status = wait_for(&engine, "erreur", |state| {
        matches!(state, UpdateState::Error { .. })
    })
    .await;
    assert_eq!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Download,
            detail: t!("updates.errors.downloadStatus", status = "404 Not Found"),
        }
    );
    assert!(files(&rig.staging).is_empty());
}

#[test]
fn plugin_errors_are_said_in_the_interface_language() {
    use tauri_plugin_updater::Error;
    assert_eq!(
        plugin_error_text(&Error::Network(
            "Download request failed with status: 404 Not Found".into()
        )),
        t!("updates.errors.downloadStatus", status = "404 Not Found")
    );
    assert_eq!(
        plugin_error_text(&Error::Network("connection reset".into())),
        t!("updates.errors.unreachable", error = "connection reset")
    );
    assert_eq!(
        plugin_error_text(&Error::MissingSignedVersion),
        t!("updates.unsignedVersion")
    );
    assert_eq!(
        plugin_error_text(&Error::SignedVersionMismatch {
            signed: "9.9.9".into(),
            announced: "9.9.10".into(),
        }),
        t!(
            "updates.signedForOtherVersion",
            signed = "9.9.9",
            version = "9.9.10"
        )
    );
    assert_eq!(
        plugin_error_text(&Error::TargetNotFound("linux-x86_64".into())),
        t!("updates.errors.invalidRelease")
    );
    assert_eq!(
        plugin_error_text(&Error::EmptyEndpoints),
        t!("updates.notConfigured")
    );
    let missing = plugin_error_text(&Error::ReleaseNotFound);
    assert_eq!(missing, t!("updates.errors.noRelease"));
    assert!(!missing.contains("Could not fetch"), "{missing}");
    // Cas sans texte à lui : le message du greffon, après un titre traduit.
    assert_eq!(
        plugin_error_text(&Error::FormatDate),
        t!("updates.errors.unexpected", error = "Failed to format date")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_check_that_fails_keeps_an_announced_update() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, false);
    let available = engine.check(Trigger::Scheduled).await.state;
    assert!(matches!(available, UpdateState::Available { .. }));

    // Hors ligne à l'heure suivante : toujours annoncée (Download), sans
    // passer par « Checking », l'échec noté à côté.
    server.served.lock().status = 500;
    let mut receiver = engine.subscribe();
    receiver.borrow_and_update();
    let status = engine.check(Trigger::Scheduled).await;
    assert_eq!(status.state, available);
    assert_eq!(
        status.last_error.as_ref().map(|error| error.code),
        Some(UpdateErrorCode::Check)
    );
    assert_eq!(receiver.borrow_and_update().state, available);
    assert_eq!(server.count("/package"), 0);

    // De nouveau en ligne : l'échec est effacé.
    server.served.lock().status = 200;
    let status = engine.check(Trigger::Scheduled).await;
    assert_eq!(status.state, available);
    assert_eq!(status.last_error, None);

    // Vérification demandée qui échoue : une erreur, montrée comme telle
    // (pas aussi dans `lastError`).
    server.served.lock().status = 500;
    let status = engine.check(Trigger::Requested).await;
    assert!(matches!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Check,
            ..
        }
    ));
    assert_eq!(status.last_error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_build_never_checks() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    for reason in [
        UnsupportedReason::NoPackage,
        UnsupportedReason::PackageManager,
    ] {
        let engine = Arc::new(Engine::new(Config {
            updater: None,
            support: Support::Unsupported(reason),
            platform: Platform {
                appimage: None,
                pkexec: None,
            },
            staging: rig.staging.clone(),
            pubkey: TEST_PUBKEY.into(),
            require_signed_version: true,
            current: Version::new(0, 1, 0),
            auto_download: true,
        }));
        engine.restore().await;
        let status = engine.check(Trigger::Requested).await;
        assert_eq!(
            status.state,
            UpdateState::Unsupported {
                reason,
                version: None
            }
        );
        assert_eq!(status.installer, None);
        assert!(engine.download().is_err());
    }
    assert!(server.requests.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_folder_is_reported_before_any_download() {
    use std::os::unix::fs::PermissionsExt;
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let folder = rig.appimage.parent().unwrap().to_owned();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Auto-download coupé : dit dès la vérification (sans attendre un clic
    // sur « Download »).
    let engine = engine(&server, &rig, false);
    let status = engine.check(Trigger::Scheduled).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let state = engine.status().state;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        status.state,
        UpdateState::Unsupported {
            reason: UnsupportedReason::ReadOnlyFolder,
            version: Some("9.9.9".into())
        }
    );
    assert_eq!(state, status.state);
    assert_eq!(server.count("/package"), 0);

    // Toujours non inscriptible : « Download » ne télécharge rien.
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = engine.download().unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(refused.state, status.state);
    assert_eq!(server.count("/package"), 0);

    // Dossier rendu inscriptible : la vérification suivante la signale,
    // puis elle se télécharge sur demande.
    let status = engine.check(Trigger::Requested).await;
    assert!(matches!(status.state, UpdateState::Available { .. }));
    engine.download().unwrap();
    wait_for(&engine, "prête", ready("9.9.9")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_checks_never_run_at_once() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    server.served.lock().delay = Duration::from_millis(400);
    let rig = rig();
    let engine = engine(&server, &rig, false);
    let (first, second) = tokio::join!(
        engine.check(Trigger::Scheduled),
        engine.check(Trigger::Requested)
    );
    assert_eq!(server.count("/latest.json"), 1);
    assert!(matches!(first.state, UpdateState::Available { .. }));
    assert_eq!(first, second);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_to_update_replaces_the_appimage_even_offline() {
    use std::os::unix::fs::PermissionsExt;
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;

    // Relancée hors ligne : la mise à jour gardée s'installe sans relire
    // l'annonce du serveur.
    let relaunched = self::engine(&server, &rig, true);
    relaunched.restore().await;
    server.served.lock().status = 500;
    let checks = server.count("/latest.json");
    let prepared = relaunched.prepare_install().await.unwrap();
    assert_eq!(server.count("/latest.json"), checks);
    // Installation en cours : dite, et une vérification n'y change rien.
    let installing = UpdateState::Installing {
        version: "9.9.9".into(),
    };
    assert_eq!(relaunched.status().state, installing);
    assert_eq!(
        crate::updater::TrayUpdate::of(&relaunched.status().state),
        crate::updater::TrayUpdate::Installing
    );
    server.served.lock().status = 200;
    assert_eq!(relaunched.check(Trigger::Requested).await.state, installing);
    server.served.lock().manifest = None;
    assert_eq!(relaunched.check(Trigger::Scheduled).await.state, installing);
    assert!(relaunched.download().is_ok());
    assert_eq!(relaunched.status().state, installing);
    // Second clic pendant l'installation : refusé.
    assert_eq!(
        relaunched.prepare_install().await.err(),
        Some(Refusal::Busy)
    );
    relaunched.install(prepared).await.unwrap();
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), test_appimage());
    let mode = std::fs::metadata(&rig.appimage)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    // Aucune copie de secours ni fichier temporaire laissé à côté.
    assert_eq!(files(rig.appimage.parent().unwrap()), ["ChatonLM.AppImage"]);

    // Au démarrage de la nouvelle version : mise à jour gardée supprimée.
    let updated = Arc::new(Engine::new(Config {
        current: Version::new(9, 9, 9),
        ..config_for(&server, &rig)
    }));
    updated.restore().await;
    assert_eq!(updated.status().state, UpdateState::Idle);
    assert!(files(&rig.staging).is_empty());
}

fn config_for(server: &FakeServer, rig: &Rig) -> Config {
    Config {
        updater: Some(plugin_updater(&server.endpoint(), &rig.appimage)),
        support: Support::Installable(UpdateInstaller::AppImage),
        platform: Platform {
            appimage: Some(rig.appimage.clone()),
            pkexec: None,
        },
        staging: rig.staging.clone(),
        pubkey: TEST_PUBKEY.into(),
        require_signed_version: true,
        current: Version::new(0, 1, 0),
        auto_download: true,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn install_is_refused_until_ready_and_rechecks_the_kept_file() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, false);
    let not_ready = Some(UpdateErrorInfo {
        code: UpdateErrorCode::Install,
        detail: t!("updates.notReady"),
    });
    assert_eq!(
        engine.prepare_install().await.err(),
        Some(Refusal::NotReady)
    );
    // Refus dit (`lastError`), jamais muet, sans changer l'état.
    assert_eq!(engine.status().state, UpdateState::Idle);
    assert_eq!(engine.status().last_error, not_ready);
    engine.check(Trigger::Scheduled).await;
    assert_eq!(engine.status().last_error, None);
    assert_eq!(
        engine.prepare_install().await.err(),
        Some(Refusal::NotReady)
    );
    assert!(matches!(
        engine.status().state,
        UpdateState::Available { .. }
    ));
    assert_eq!(engine.status().last_error, not_ready);
    engine.download().unwrap();
    wait_for(&engine, "prête", ready("9.9.9")).await;

    // Paquet changé sur le disque après le téléchargement : refusé, supprimé.
    std::fs::write(rig.staging.join("chatonlm-9.9.9.update"), b"evil").unwrap();
    assert!(matches!(
        engine.prepare_install().await.err(),
        Some(Refusal::Failed(_))
    ));
    assert!(matches!(
        engine.status().state,
        UpdateState::Error {
            code: UpdateErrorCode::Signature,
            ..
        }
    ));
    assert!(files(&rig.staging).is_empty());
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), OLD_APPIMAGE);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_installation_is_shown_after_the_relaunch() {
    use std::os::unix::fs::PermissionsExt;
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;
    let prepared = engine.prepare_install().await.unwrap();
    // Dossier devenu non inscriptible entre la préparation et l'installation.
    let folder = rig.appimage.parent().unwrap().to_owned();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = engine.install(prepared).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), OLD_APPIMAGE);
    // Aucun fichier temporaire laissé (dossier non inscriptible : rien n'a
    // pu y être écrit).
    assert_eq!(files(&folder), ["ChatonLM.AppImage"]);

    // Relance (même version) : l'échec est montré, et le reste aux
    // vérifications automatiques ; une vérification demandée repasse à
    // « prête » (sans nouveau téléchargement).
    let relaunched = self::engine(&server, &rig, true);
    relaunched.restore().await;
    let UpdateState::Error { code, detail } = relaunched.status().state else {
        panic!("{:?}", relaunched.status().state)
    };
    assert_eq!(code, UpdateErrorCode::Install);
    assert_eq!(Err(detail), result);
    let status = relaunched.check(Trigger::Scheduled).await;
    assert!(matches!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Install,
            ..
        }
    ));
    let status = relaunched.check(Trigger::Requested).await;
    assert!(ready("9.9.9")(&status.state));
    assert_eq!(server.count("/package"), 1);
    // Échec lu une fois.
    let again = self::engine(&server, &rig, true);
    again.restore().await;
    assert!(ready("9.9.9")(&again.status().state));
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_drops_an_installed_or_damaged_update() {
    let rig = rig();
    let staged = Staged::new("9.9.9", None, APPIMAGE_SIGNATURE, UpdateInstaller::AppImage);
    let server = FakeServer::start().await;

    // Déjà installée (l'app est en 9.9.9).
    staging::save(&rig.staging, &staged, &test_appimage()).unwrap();
    let engine = Arc::new(Engine::new(Config {
        current: Version::new(9, 9, 9),
        ..config_for(&server, &rig)
    }));
    engine.restore().await;
    assert_eq!(engine.status().state, UpdateState::Idle);
    assert!(files(&rig.staging).is_empty());

    // Abîmée.
    staging::save(&rig.staging, &staged, b"damaged").unwrap();
    let engine = self::engine(&server, &rig, true);
    engine.restore().await;
    assert_eq!(engine.status().state, UpdateState::Idle);
    assert!(files(&rig.staging).is_empty());
    assert!(server.requests.lock().is_empty());
}

#[test]
fn a_missing_updater_is_said() {
    let engine = Engine::new(Config {
        updater: None,
        support: Support::Installable(UpdateInstaller::Deb),
        platform: Platform {
            appimage: None,
            pkexec: None,
        },
        staging: PathBuf::from("/nonexistent"),
        pubkey: String::new(),
        require_signed_version: true,
        current: Version::new(0, 1, 0),
        auto_download: true,
    });
    assert_eq!(
        engine.status().state,
        UpdateState::Error {
            code: UpdateErrorCode::Check,
            detail: t!("updates.notConfigured"),
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_restart_keeps_the_update_ready_and_says_why() {
    use std::os::unix::fs::PermissionsExt;
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;

    let folder = rig.appimage.parent().unwrap().to_owned();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = engine.prepare_install().await.err();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(refused, Some(Refusal::ReadOnlyFolder));
    let status = engine.status();
    assert!(ready("9.9.9")(&status.state));
    assert_eq!(
        status.last_error,
        Some(UpdateErrorInfo {
            code: UpdateErrorCode::Install,
            detail: t!("updates.readOnlyFolder"),
        })
    );
    assert_eq!(
        files(&rig.staging),
        ["chatonlm-9.9.9.update", "update.json"]
    );

    // Vérification réussie suivante : toujours prête, refus effacé ; le
    // dossier redevenu inscriptible, l'installation est acceptée.
    let status = engine.check(Trigger::Scheduled).await;
    assert!(ready("9.9.9")(&status.state));
    assert_eq!(status.last_error, None);
    assert!(engine.prepare_install().await.is_ok());
    assert!(matches!(
        engine.status().state,
        UpdateState::Installing { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signed_package_of_another_type_is_never_kept() {
    // Le .deb signé (nom signé « .deb ») annoncé à une AppImage : la
    // signature est bonne, le paquet refusé, l'AppImage intacte.
    let server = FakeServer::start().await;
    server.serve(test_deb(), DEB_SIGNATURE);
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    let status = wait_for(&engine, "erreur", |state| {
        matches!(state, UpdateState::Error { .. })
    })
    .await;
    assert_eq!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Signature,
            detail: t!(
                "updates.signedForOtherFile",
                file = "chatonlm_9.9.9_amd64.deb"
            ),
        }
    );
    assert!(files(&rig.staging).is_empty());
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), OLD_APPIMAGE);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_kept_update_belongs_to_the_package_type_that_downloaded_it() {
    // AppImage et .deb partagent le dossier des données : la mise à jour
    // gardée par l'une n'est jamais reprise par l'autre.
    let server = FakeServer::start().await;
    let rig = rig();
    let deb = |server: &FakeServer, rig: &Rig| Config {
        support: Support::Installable(UpdateInstaller::Deb),
        platform: Platform::default(),
        ..config_for(server, rig)
    };
    let appimage = Staged::new("9.9.9", None, APPIMAGE_SIGNATURE, UpdateInstaller::AppImage);
    staging::save(&rig.staging, &appimage, &test_appimage()).unwrap();
    let engine = Arc::new(Engine::new(deb(&server, &rig)));
    engine.restore().await;
    assert_eq!(engine.status().state, UpdateState::Idle);
    assert!(files(&rig.staging).is_empty());

    let kept = Staged::new("9.9.9", None, DEB_SIGNATURE, UpdateInstaller::Deb);
    staging::save(&rig.staging, &kept, &test_deb()).unwrap();
    let engine = self::engine(&server, &rig, true);
    engine.restore().await;
    assert_eq!(engine.status().state, UpdateState::Idle);
    assert!(files(&rig.staging).is_empty());

    // Le .deb reprend bien le sien.
    staging::save(&rig.staging, &kept, &test_deb()).unwrap();
    let engine = Arc::new(Engine::new(deb(&server, &rig)));
    engine.restore().await;
    assert!(ready("9.9.9")(&engine.status().state));
    assert!(server.requests.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_package_is_not_downloaded_again_by_scheduled_checks() {
    let server = FakeServer::start().await;
    let mut tampered = test_appimage();
    tampered[4242] ^= 0x80;
    server.serve(tampered.clone(), APPIMAGE_SIGNATURE);
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    let refused = |state: &UpdateState| {
        matches!(
            state,
            UpdateState::Error {
                code: UpdateErrorCode::Signature,
                ..
            }
        )
    };
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "refusée", refused).await;
    assert_eq!(server.count("/package"), 1);

    // Même annonce à l'heure suivante : rien de retéléchargé, l'erreur
    // reste (sans passer par « Checking »).
    let status = engine.check(Trigger::Scheduled).await;
    assert!(refused(&status.state), "{:?}", status.state);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(refused(&engine.status().state));
    assert_eq!(server.count("/package"), 1);

    // Retry : retéléchargée.
    engine.check(Trigger::Requested).await;
    wait_for(&engine, "refusée", refused).await;
    assert_eq!(server.count("/package"), 2);

    // Nouvelle annonce (autre signature) : retéléchargée aussi.
    server.serve(tampered, APPIMAGE_RESIGNED);
    server.announce("9.9.9");
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "refusée", refused).await;
    assert_eq!(server.count("/package"), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_release_republished_under_the_same_version_is_downloaded_again() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;

    // Même numéro, nouvelle construction (nouvelle signature).
    server.serve(test_appimage(), APPIMAGE_RESIGNED);
    server.announce("9.9.9");
    let relaunched = self::engine(&server, &rig, true);
    relaunched.restore().await;
    relaunched.check(Trigger::Scheduled).await;
    wait_for(&relaunched, "prête", ready("9.9.9")).await;
    assert_eq!(server.count("/package"), 2);
    assert_eq!(
        staging::load(&rig.staging).unwrap().signature,
        APPIMAGE_RESIGNED
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_update_that_cannot_be_read_is_not_a_bad_signature() {
    let server = FakeServer::start().await;
    server.announce("9.9.9");
    let rig = rig();
    let engine = engine(&server, &rig, true);
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;

    // Supprimée par un nettoyeur de disque : erreur de lecture.
    std::fs::remove_file(rig.staging.join("chatonlm-9.9.9.update")).unwrap();
    assert!(matches!(
        engine.prepare_install().await.err(),
        Some(Refusal::Failed(_))
    ));
    assert!(matches!(
        engine.status().state,
        UpdateState::Error {
            code: UpdateErrorCode::Io,
            ..
        }
    ));
    assert!(files(&rig.staging).is_empty());
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), OLD_APPIMAGE);
    // Une vérification la retélécharge.
    engine.check(Trigger::Requested).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;
    assert_eq!(server.count("/package"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deb_is_installed_by_pkexec_only() {
    let server = FakeServer::start().await;
    server.serve(test_deb(), DEB_SIGNATURE);
    server.announce("9.9.9");
    let rig = rig();
    let fake = crate::updater::install::linux::tests::FakePkexec::new(126);
    let engine = Arc::new(Engine::new(Config {
        support: Support::Installable(UpdateInstaller::Deb),
        platform: Platform {
            appimage: None,
            pkexec: Some(fake.program.clone()),
        },
        ..config_for(&server, &rig)
    }));
    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;
    assert_eq!(
        staging::load(&rig.staging).unwrap().installer,
        UpdateInstaller::Deb
    );
    let prepared = engine.prepare_install().await.unwrap();
    // Boîte de mot de passe fermée : erreur claire, un seul appel (aucun
    // repli), échec noté pour la relance.
    let result = engine.install(prepared).await;
    assert_eq!(result, Err(t!("updates.deb.dismissed")));
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(fake.received(), test_deb());
    assert_eq!(
        engine.status().state,
        UpdateState::Error {
            code: UpdateErrorCode::Install,
            detail: t!("updates.deb.dismissed"),
        }
    );
    assert_eq!(std::fs::read(&rig.appimage).unwrap(), OLD_APPIMAGE);
    let relaunched = Arc::new(Engine::new(Config {
        support: Support::Installable(UpdateInstaller::Deb),
        platform: Platform {
            appimage: None,
            pkexec: Some(fake.program.clone()),
        },
        ..config_for(&server, &rig)
    }));
    relaunched.restore().await;
    assert!(matches!(
        relaunched.status().state,
        UpdateState::Error {
            code: UpdateErrorCode::Install,
            ..
        }
    ));
}
