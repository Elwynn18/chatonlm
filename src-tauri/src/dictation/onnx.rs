//! ONNX Runtime de Parakeet (étape 18a), chargé à l'exécution (`ort`,
//! fonctionnalité `load-dynamic`) : le build officiel de Microsoft
//! (`packaging/onnxruntime.env`, glibc ≥ 2.27), et non les binaires statiques
//! de pyke (glibc ≥ 2.38, impossibles à lier sous Ubuntu 22.04).
//!
//! Chargé une seule fois, à la première demande (catalogue des modèles,
//! dictée, chargement de Parakeet), avant tout autre appel à `ort` :
//! `ORT_DYLIB_PATH` s'il est défini (builds sans paquet : activate.sh, CI),
//! sinon la bibliothèque du dossier des ressources de Tauri, installée par
//! les paquets (nom et emplacement par système : `onnx/linux.rs`,
//! `onnx/windows.rs`). Un échec (fichier absent, mauvaise version, chargement
//! refusé) rend Parakeet indisponible, avec un message clair : `ort` n'est
//! alors plus jamais appelé (il s'arrêterait sur une bibliothèque
//! introuvable), et l'app continue.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use ts_rs::TS;

use crate::i18n::t;

#[cfg(target_os = "linux")]
#[path = "onnx/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "onnx/windows.rs"]
mod os;

pub use os::LIBRARY;

/// Variable d'`ort` : chemin de la bibliothèque.
const DYLIB_ENV: &str = "ORT_DYLIB_PATH";

/// Pourquoi ONNX Runtime n'a pas pu être chargé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RuntimeProblemKind {
    /// Fichier absent.
    Missing,
    /// Fichier chargé, mais pas un ONNX Runtime utilisable (version trop
    /// ancienne, `OrtGetApiBase` absent).
    Incompatible,
    /// Chargement refusé par le système (dépendance ou glibc trop ancienne,
    /// runtime MSVC absent sous Windows, fichier abîmé...).
    LoadFailed,
}

/// ONNX Runtime indisponible : Parakeet ne peut pas tourner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeProblem {
    pub kind: RuntimeProblemKind,
    /// Détail technique, en anglais (chemin, erreur du système).
    pub detail: String,
}

impl RuntimeProblem {
    fn new(kind: RuntimeProblemKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// Dossier des ressources de l'app (fixé au démarrage).
static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Résultat du chargement (une seule tentative par lancement).
static LOADED: OnceLock<Result<(), RuntimeProblem>> = OnceLock::new();

/// Démarrage de l'app : dossier des ressources, où les paquets installent la
/// bibliothèque.
pub fn configure(resource_dir: PathBuf) {
    let _ = RESOURCE_DIR.set(resource_dir);
}

/// Bibliothèque à charger : `ORT_DYLIB_PATH` non vide (relatif : au dossier
/// du programme, comme `ort`), sinon [`LIBRARY`] du dossier des ressources.
fn library_path(
    env: Option<OsString>,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Option<PathBuf> {
    match env.filter(|value| !value.is_empty()).map(PathBuf::from) {
        Some(path) if path.is_absolute() => Some(path),
        Some(path) => Some(exe_dir.map_or(path.clone(), |dir| dir.join(&path))),
        None => resource_dir.map(|dir| dir.join(LIBRARY)),
    }
}

/// Chemin sans le préfixe `\\?\` que Windows donne aux chemins canonisés
/// (celui du dossier des ressources de Tauri) : `\\?\C:\…` devient `C:\…`,
/// lisible dans les messages. Gardé tel quel s'il n'a pas cette forme, ou
/// s'il est trop long pour s'en passer (260 caractères).
fn without_verbatim(path: PathBuf) -> PathBuf {
    let plain = path.to_str().and_then(|text| {
        let rest = text.strip_prefix(r"\\?\")?;
        let bytes = rest.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\';
        (drive && rest.len() < 260).then(|| PathBuf::from(rest))
    });
    plain.unwrap_or(path)
}

/// Classe une erreur de chargement d'`ort`.
fn classify(error: &ort::LoadDynamicError) -> RuntimeProblem {
    let kind = match error {
        ort::LoadDynamicError::Dlopen { .. } => RuntimeProblemKind::LoadFailed,
        ort::LoadDynamicError::MissingApi { .. } | ort::LoadDynamicError::BadVersion { .. } => {
            RuntimeProblemKind::Incompatible
        }
    };
    RuntimeProblem::new(kind, error.to_string())
}

/// Charge `path` par `init` (`ort::init_from` en vrai), après avoir vérifié
/// qu'il existe.
fn load_with(
    path: Option<PathBuf>,
    init: impl FnOnce(&Path) -> Result<(), ort::LoadDynamicError>,
) -> Result<(), RuntimeProblem> {
    let Some(path) = path else {
        return Err(RuntimeProblem::new(
            RuntimeProblemKind::Missing,
            "no ONNX Runtime library location",
        ));
    };
    if !path.is_file() {
        return Err(RuntimeProblem::new(
            RuntimeProblemKind::Missing,
            path.display().to_string(),
        ));
    }
    init(&path).map_err(|error| classify(&error))
}

/// ONNX Runtime chargé (bloquant la première fois : à appeler hors de la
/// boucle des évènements) ; sinon pourquoi. Avant tout autre appel à `ort`,
/// et une seule tentative : après un échec, le `OnceLock` interne d'`ort`
/// (rc.13, `call_once_force`) se croit rempli sans valeur ; un nouvel
/// `init_from` répondrait « chargé » et le moindre appel lirait une
/// bibliothèque inexistante. Parakeet n'est donc plus jamais chargé.
pub fn ensure() -> Result<(), RuntimeProblem> {
    LOADED
        .get_or_init(|| {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf));
            let path = library_path(
                std::env::var_os(DYLIB_ENV),
                RESOURCE_DIR.get().map(PathBuf::as_path),
                exe_dir.as_deref(),
            );
            // Windows : chemin lisible dans les messages (et toujours absolu).
            let path = if cfg!(windows) {
                path.map(without_verbatim)
            } else {
                path
            };
            let loaded = load_with(path, |path| {
                ort::init_from(path).map(|environment| {
                    environment.commit();
                })
            });
            match &loaded {
                Ok(()) => eprintln!("chatonlm: ONNX Runtime loaded"),
                Err(problem) => eprintln!("chatonlm: ONNX Runtime unavailable: {}", problem.detail),
            }
            loaded
        })
        .clone()
}

/// Refus de Parakeet sans ONNX Runtime (dictée, chargement, téléchargement).
pub fn unavailable(problem: &RuntimeProblem) -> String {
    t!("dictation.runtimeUnavailable", detail = problem.detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Racine d'un chemin absolu du système (`/x` n'est pas absolu sous
    /// Windows).
    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" })
    }

    #[test]
    fn the_variable_comes_first_then_the_resources() {
        let resources = root().join("usr").join("lib").join("chatonlm");
        let exe = root().join("opt").join("chatonlm").join("bin");
        let (resources, exe) = (resources.as_path(), exe.as_path());
        assert_eq!(
            library_path(None, Some(resources), Some(exe)),
            Some(resources.join(LIBRARY))
        );
        // Vide : comme absente.
        assert_eq!(
            library_path(Some("".into()), Some(resources), Some(exe)),
            Some(resources.join(LIBRARY))
        );
        let elsewhere = root().join("x").join(LIBRARY);
        assert!(elsewhere.is_absolute());
        assert_eq!(
            library_path(
                Some(elsewhere.clone().into_os_string()),
                Some(resources),
                Some(exe)
            ),
            Some(elsewhere)
        );
        // Relatif : au dossier du programme, comme `ort`.
        assert_eq!(
            library_path(Some("lib/ort.so".into()), Some(resources), Some(exe)),
            Some(exe.join("lib/ort.so"))
        );
        assert_eq!(library_path(None, None, Some(exe)), None);
    }

    #[test]
    fn the_library_has_the_name_of_the_system() {
        if cfg!(windows) {
            assert_eq!(LIBRARY, "onnxruntime.dll");
        } else {
            assert_eq!(LIBRARY, "libonnxruntime.so.1");
        }
    }

    /// Chemins canonisés de Windows (logique pure, essayée partout).
    #[test]
    fn the_verbatim_prefix_is_removed_from_drive_paths_only() {
        let plain = |path: &str| without_verbatim(PathBuf::from(path));
        assert_eq!(
            plain(r"\\?\C:\Users\Théo\AppData\Local\chatonlm\onnxruntime.dll"),
            PathBuf::from(r"C:\Users\Théo\AppData\Local\chatonlm\onnxruntime.dll")
        );
        // Déjà simple, partage réseau, chemin Unix : inchangés.
        for kept in [
            r"C:\chatonlm\onnxruntime.dll",
            r"\\?\UNC\server\share\onnxruntime.dll",
            r"\\?\Volume{b75e2c83}\onnxruntime.dll",
            "/usr/lib/chatonlm/libonnxruntime.so.1",
        ] {
            assert_eq!(plain(kept), PathBuf::from(kept));
        }
        // Trop long sans le préfixe : gardé (lui seul passe les 260 caractères).
        let long = format!(r"\\?\C:\{}\onnxruntime.dll", "a".repeat(260));
        assert_eq!(plain(&long), PathBuf::from(&long));
    }

    #[test]
    fn a_missing_file_is_never_handed_to_ort() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join(LIBRARY);
        let refused = load_with(Some(absent.clone()), |_| {
            panic!("ort called for a missing file")
        })
        .unwrap_err();
        assert_eq!(
            refused,
            RuntimeProblem::new(RuntimeProblemKind::Missing, absent.display().to_string())
        );
        let nowhere = load_with(None, |_| panic!("ort called without a path")).unwrap_err();
        assert_eq!(nowhere.kind, RuntimeProblemKind::Missing);
        // Présent : chargé.
        std::fs::write(&absent, b"").unwrap();
        assert_eq!(load_with(Some(absent), |_| Ok(())), Ok(()));
    }

    /// Erreur réelle d'`ort` (fichier qui n'est pas une bibliothèque), sans
    /// jamais charger ONNX Runtime dans le processus des essais. Un seul
    /// appel à `ort::init_from` par processus : après un échec, `ort` se
    /// croit chargé (voir [`ensure`]).
    #[test]
    fn load_failures_are_classified_without_crashing() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join(LIBRARY);
        std::fs::write(&text, b"not a library").unwrap();
        let broken = load_with(Some(text.clone()), |path| {
            ort::init_from(path).map(|environment| {
                environment.commit();
            })
        })
        .unwrap_err();
        assert_eq!(broken.kind, RuntimeProblemKind::LoadFailed, "{broken:?}");
        assert!(
            broken.detail.contains(&*text.to_string_lossy()),
            "{broken:?}"
        );
        // Bibliothèque sans OrtGetApiBase, ou trop ancienne : incompatible.
        let missing_api = ort::LoadDynamicError::MissingApi { path: text.clone() };
        assert_eq!(
            classify(&missing_api).kind,
            RuntimeProblemKind::Incompatible
        );
        let old = ort::LoadDynamicError::BadVersion {
            version_str: "1.16.3".into(),
            path: text,
        };
        let old = classify(&old);
        assert_eq!(old.kind, RuntimeProblemKind::Incompatible);
        assert!(old.detail.contains("1.16.3"), "{old:?}");
    }

    #[test]
    fn the_refusal_gives_the_detail_in_every_language() {
        let problem = RuntimeProblem::new(RuntimeProblemKind::Missing, "/x/libonnxruntime.so.1");
        for lang in [
            crate::i18n::Lang::En,
            crate::i18n::Lang::Es,
            crate::i18n::Lang::Fr,
        ] {
            let message = t!("dictation.runtimeUnavailable" in lang, detail = problem.detail);
            assert!(message.contains("/x/libonnxruntime.so.1"), "{message}");
            assert!(message.contains("ONNX Runtime"), "{message}");
        }
        assert!(unavailable(&problem).contains("/x/libonnxruntime.so.1"));
    }
}
