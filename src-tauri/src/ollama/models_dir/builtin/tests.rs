//! Essais du dossier des modèles de l'Ollama intégré : lecture, déplacement,
//! remise en place, et changement complet contre le faux `ollama serve`.

use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use super::*;
use crate::ollama::builtin::tests::{fake_config, temp_dir};
#[cfg(unix)]
use crate::ollama::models_dir::{FolderProblem, check_folder_as};

/// Dossier de modèles : un manifeste, deux blobs (3 et 5 octets) et un
/// téléchargement inachevé.
fn models(dir: &Path) {
    let manifest = dir.join("manifests/registry.ollama.ai/library/qwen3/0.6b");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(&manifest, "{}").unwrap();
    fs::create_dir_all(dir.join("blobs")).unwrap();
    fs::write(dir.join("blobs/sha256-aaa"), "abc").unwrap();
    fs::write(dir.join("blobs/sha256-bbb"), "defgh").unwrap();
    fs::write(dir.join("blobs/sha256-ccc-partial"), "x").unwrap();
}

#[cfg(unix)]
fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self").unwrap().uid()
}

/// Sous Windows, les dossiers n'ont pas de propriétaire à comparer : la
/// vérification ordinaire.
#[cfg(windows)]
fn own_uid() -> u32 {
    0
}

#[cfg(windows)]
fn check_folder_as(raw: &str, detection: &Detection, _own_uid: u32) -> FolderCheck {
    crate::ollama::models_dir::check_folder(raw, detection)
}

#[test]
fn models_are_counted_on_the_disk() {
    let dir = temp_dir();
    // Rien (ou pas de dossier) : zéro, pas « inconnu ».
    let none = ModelsSummary { count: 0, bytes: 0 };
    assert_eq!(folder_summary(&dir.path().join("missing")), none);
    assert_eq!(folder_summary(dir.path()), none);
    models(dir.path());
    assert_eq!(
        folder_summary(dir.path()),
        ModelsSummary { count: 1, bytes: 8 }
    );
}

/// Même modèle, autre contenu, dans le nouveau dossier : la fusion
/// effacerait (au redémarrage, par Ollama) les couches de la version
/// écrasée. Refusée ; un manifeste identique est simplement gardé.
#[test]
fn a_different_model_of_the_same_name_is_never_merged() {
    let dir = temp_dir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    models(&old);
    let manifest = "manifests/registry.ollama.ai/library/qwen3/0.6b";
    fs::create_dir_all(new.join(manifest).parent().unwrap()).unwrap();
    fs::write(new.join(manifest), "{}").unwrap();
    let entries = plan(&old, &new).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.skipped && entry.relative == Path::new(manifest))
    );
    fs::write(new.join(manifest), r#"{"layers":[]}"#).unwrap();
    match plan(&old, &new) {
        Err(PlanError::Conflict { model }) => assert_eq!(model, "qwen3:0.6b"),
        other => panic!("{other:?}"),
    }
    let other = Path::new("manifests/hf.co/user/repo/Q4_K_M");
    assert_eq!(model_name(other), "hf.co/user/repo:Q4_K_M");
    let user = Path::new("manifests/registry.ollama.ai/me/mine/latest");
    assert_eq!(model_name(user), "me/mine:latest");
}

/// Lien symbolique (blob partagé avec un autre Ollama) : ni oublié ni
/// déplacé à moitié, refusé.
#[cfg(unix)]
#[test]
fn a_symbolic_link_is_refused() {
    let dir = temp_dir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    models(&old);
    let shared = dir.path().join("shared-blob");
    fs::write(&shared, "shared").unwrap();
    std::os::unix::fs::symlink(&shared, old.join("blobs/sha256-ddd")).unwrap();
    match plan(&old, &new) {
        Err(PlanError::Special { path }) => assert_eq!(path, Path::new("blobs/sha256-ddd")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn renamed_on_the_same_drive_and_merged() {
    let dir = temp_dir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    models(&old);
    fs::create_dir_all(new.join("blobs")).unwrap();
    fs::write(new.join("blobs/sha256-aaa"), "abc").unwrap();
    let entries = plan(&old, &new).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries.iter().filter(|entry| entry.skipped).count(), 1);
    assert_eq!(to_copy(&entries), 5 + 1 + 2);
    let done = AtomicU64::new(0);
    let moved = move_files(&old, &new, &entries, true, &done).unwrap();
    assert_eq!(
        (moved.renamed.len(), moved.copied.len(), moved.skipped),
        (3, 0, 1)
    );
    assert_eq!(done.load(Ordering::Relaxed), 0);
    assert_eq!(
        fs::read_to_string(new.join("blobs/sha256-bbb")).unwrap(),
        "defgh"
    );
    assert!(cleanup(&old, &moved));
    // L'ancien dossier ne garde que le fichier déjà présent ailleurs.
    assert!(old.join("blobs/sha256-aaa").exists());
    assert!(!old.join("manifests").exists());
    assert_eq!(folder_summary(&new).count, 1);
}

#[test]
fn copied_to_another_drive_with_progress_then_removed() {
    let dir = temp_dir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    models(&old);
    let entries = plan(&old, &new).unwrap();
    let done = AtomicU64::new(0);
    let moved = move_files(&old, &new, &entries, false, &done).unwrap();
    assert_eq!((moved.renamed.len(), moved.copied.len()), (0, 4));
    assert_eq!(done.load(Ordering::Relaxed), to_copy(&entries));
    // Copies complètes, sans fichier partiel ; l'ancien dossier intact
    // jusqu'au nettoyage.
    assert_eq!(
        fs::read_to_string(new.join("blobs/sha256-aaa")).unwrap(),
        "abc"
    );
    assert!(!new.join(format!("blobs/sha256-aaa{PARTIAL}")).exists());
    assert!(old.join("blobs/sha256-aaa").exists());
    assert!(cleanup(&old, &moved));
    assert!(!old.exists());
}

#[test]
fn a_failure_puts_everything_back() {
    let dir = temp_dir();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    models(&old);
    // `manifests` est un fichier dans le nouveau dossier : les blobs passent,
    // puis le manifeste échoue.
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("manifests"), "").unwrap();
    let entries = plan(&old, &new).unwrap();
    for same_drive in [true, false] {
        let done = AtomicU64::new(0);
        let error = move_files(&old, &new, &entries, same_drive, &done).unwrap_err();
        assert!(error.restored, "{error:?}");
        assert!(error.detail.contains("manifests"), "{}", error.detail);
        assert_eq!(folder_summary(&old).bytes, 8);
        assert!(!new.join("blobs").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_builtin_folder_changes_with_its_models() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let old = builtin.config().default_models_dir();
    models(&old);
    builtin.ensure_started(&old).unwrap();
    assert_eq!(
        builtin.wait_ready(Duration::from_secs(20)).await.state,
        BuiltinState::Running
    );
    let new = dir.path().join("elsewhere");
    let check = check_folder_as(new.to_str().unwrap(), &detection(&old, true), own_uid());
    assert_eq!(check.problem, None);
    assert!(check.same_drive);
    let mut events = Vec::new();
    let mut stopped = false;
    let mut saved = None;
    let outcome = change(
        &builtin,
        Change {
            old: old.clone(),
            check,
            move_models: true,
        },
        true,
        |event| events.push(event),
        || stopped = true,
        |path| {
            saved = Some(path);
            async { Ok(()) }
        },
    )
    .await;
    assert_eq!(
        outcome,
        MoveOutcome::Done {
            restarted: true,
            renamed: 4,
            copied: 0,
            skipped: 0,
            cleanup_failed: false,
        }
    );
    assert!(stopped);
    let new = fs::canonicalize(&new).unwrap();
    // Le chemin gardé est sans le préfixe `\\?\` de `canonicalize`.
    #[cfg(windows)]
    let new = dunce::simplified(&new).to_path_buf();
    assert_eq!(saved, Some(new.clone()));
    let steps: Vec<MoveStep> = events
        .iter()
        .filter_map(|event| match event {
            MoveEvent::Step { step } => Some(*step),
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        [
            MoveStep::Prepare,
            MoveStep::Stop,
            MoveStep::Move,
            MoveStep::Config,
            MoveStep::Start,
            MoveStep::Cleanup
        ]
    );
    assert!(events.contains(&MoveEvent::Total { bytes: 0 }));
    // Relancé sur le nouveau dossier, l'ancien vidé.
    let snapshot = builtin.snapshot();
    assert_eq!(snapshot.state, BuiltinState::Running);
    assert_eq!(snapshot.models_dir.as_deref(), Some(new.as_path()));
    let environment = fs::read_to_string(builtin.config().home().join("env")).unwrap_or_default();
    assert!(
        environment.contains(&format!("OLLAMA_MODELS={}", new.display())),
        "{environment}"
    );
    assert_eq!(folder_summary(&new).count, 1);
    assert!(!old.exists());
    builtin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn not_enough_space_changes_nothing() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let old = dir.path().join("old");
    models(&old);
    let new = dir.path().join("new");
    let mut check = check_folder_as(new.to_str().unwrap(), &detection(&old, false), own_uid());
    check.same_drive = false;
    check.free_bytes = Some(SPACE_MARGIN);
    let mut saved = false;
    let outcome = change(
        &builtin,
        Change {
            old: old.clone(),
            check,
            move_models: true,
        },
        true,
        |_| {},
        || {},
        |_| {
            saved = true;
            async { Ok(()) }
        },
    )
    .await;
    assert_eq!(
        outcome,
        MoveOutcome::Failed {
            reason: MoveFailure::NoSpace {
                need: Some(11),
                free: Some(SPACE_MARGIN)
            },
            restored: false,
            detail: String::new(),
        }
    );
    assert!(!saved);
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert_eq!(folder_summary(&old).bytes, 8);
}

#[cfg(unix)]
#[test]
fn folders_of_another_account_or_the_same_one_are_refused() {
    let dir = temp_dir();
    let old = dir.path().join("models");
    fs::create_dir_all(&old).unwrap();
    let detection = detection(&old, true);
    let same = check_folder_as(old.to_str().unwrap(), &detection, own_uid());
    assert_eq!(same.problem, Some(FolderProblem::Same));
    // Dossier existant vu comme celui d'un autre compte.
    let other = dir.path().join("other");
    fs::create_dir_all(&other).unwrap();
    let foreign = check_folder_as(other.to_str().unwrap(), &detection, own_uid() + 1);
    assert_eq!(foreign.problem, Some(FolderProblem::NotYours));
    assert_eq!(foreign.commands, None);
}

/// Intégré qui n'est plus la cible (Ollama de la machine ou adresse
/// réglée) : arrêté pour le changement, pas relancé.
#[tokio::test(flavor = "multi_thread")]
async fn a_builtin_that_no_longer_serves_is_not_restarted() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let old = builtin.config().default_models_dir();
    models(&old);
    builtin.ensure_started(&old).unwrap();
    assert_eq!(
        builtin.wait_ready(Duration::from_secs(20)).await.state,
        BuiltinState::Running
    );
    let new = dir.path().join("elsewhere");
    let check = check_folder_as(new.to_str().unwrap(), &detection(&old, true), own_uid());
    let outcome = change(
        &builtin,
        Change {
            old,
            check,
            move_models: true,
        },
        false,
        |_| {},
        || {},
        |_| async { Ok(()) },
    )
    .await;
    assert!(
        matches!(
            outcome,
            MoveOutcome::Done {
                restarted: false,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
}

/// Fusion refusée (même modèle, autre contenu ; lien symbolique) : avant
/// tout arrêt ni déplacement, avec la raison et un détail lisible.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_merge_changes_nothing() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let old = builtin.config().default_models_dir();
    models(&old);
    builtin.ensure_started(&old).unwrap();
    let pid = builtin
        .wait_ready(Duration::from_secs(20))
        .await
        .pid
        .unwrap();
    let new = dir.path().join("elsewhere");
    let manifest = "manifests/registry.ollama.ai/library/qwen3/0.6b";
    fs::create_dir_all(new.join(manifest).parent().unwrap()).unwrap();
    fs::write(new.join(manifest), r#"{"layers":[]}"#).unwrap();
    let attempt = |check| {
        let (builtin, old) = (builtin.clone(), old.clone());
        async move {
            let mut stopped = false;
            let mut saved = false;
            let outcome = change(
                &builtin,
                Change {
                    old,
                    check,
                    move_models: true,
                },
                true,
                |_| {},
                || stopped = true,
                |_| {
                    saved = true;
                    async { Ok(()) }
                },
            )
            .await;
            assert!(!stopped && !saved);
            outcome
        }
    };
    let check = check_folder_as(new.to_str().unwrap(), &detection(&old, true), own_uid());
    let MoveOutcome::Failed { reason, detail, .. } = attempt(check).await else {
        panic!("merged");
    };
    assert_eq!(
        reason,
        MoveFailure::ModelConflict {
            model: "qwen3:0.6b".into()
        }
    );
    assert!(detail.contains("qwen3:0.6b"), "{detail}");
    // Rien n'a bougé, l'intégré tourne toujours.
    assert_eq!(folder_summary(&old), ModelsSummary { count: 1, bytes: 8 });
    assert_eq!(
        fs::read_to_string(new.join(manifest)).unwrap(),
        r#"{"layers":[]}"#
    );
    assert!(!new.join("blobs").exists());
    assert_eq!(builtin.snapshot().pid, Some(pid));

    // Liens symboliques : Unix (leur création demande un droit à part
    // sous Windows).
    #[cfg(unix)]
    {
        fs::remove_dir_all(&new).unwrap();
        let link = old.join("blobs/sha256-ddd");
        std::os::unix::fs::symlink(dir.path().join("shared"), &link).unwrap();
        let check = check_folder_as(new.to_str().unwrap(), &detection(&old, true), own_uid());
        let MoveOutcome::Failed { reason, detail, .. } = attempt(check).await else {
            panic!("moved");
        };
        let path = link.to_string_lossy().into_owned();
        assert_eq!(reason, MoveFailure::Symlink { path: path.clone() });
        assert!(detail.contains(&path), "{detail}");
        assert!(old.join("manifests").exists() && !new.exists());
    }
    assert_eq!(builtin.snapshot().pid, Some(pid));
    builtin.stop().await;
}
