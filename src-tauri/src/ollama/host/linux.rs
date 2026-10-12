//! Installation d'Ollama sous Linux : binaire et service systemd.
//!
//! Rien n'est exécuté : le binaire est cherché dans le PATH et l'état du
//! service est lu par D-Bus (bus système, puis bus de session pour un service
//! de l'utilisateur, sans droits particuliers). C'est fiable même depuis
//! l'AppImage, dont l'environnement (LD_LIBRARY_PATH...) perturberait un
//! systemctl lancé en sous-processus. Le démarrage du service (« Start
//! Ollama ») passe aussi par D-Bus ([`start_unit`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use zbus::blocking::Connection;
use zbus::blocking::proxy::Builder;
use zbus::proxy::CacheProperties;

use super::{Service, StartResult, Unit};

const UNIT_NAME: &str = "ollama.service";

/// Cherche un exécutable `ollama` dans le PATH, puis aux emplacements usuels.
pub fn find_binary() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from))
        .map(|dir| dir.join("ollama"))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Lit l'état de `ollama.service`, service du système d'abord, puis de
/// l'utilisateur. Bloquant : à appeler hors du runtime asynchrone.
pub fn ollama_unit() -> Unit {
    let system = read_unit(Bus::System, UNIT_NAME);
    if let Ok(Unit::Loaded(_)) = system {
        return system.unwrap_or(Unit::Unknown);
    }
    match read_unit(Bus::Session, UNIT_NAME) {
        Ok(Unit::Loaded(service)) => Unit::Loaded(service),
        user if matches!(system, Ok(Unit::NotFound)) || matches!(user, Ok(Unit::NotFound)) => {
            Unit::NotFound
        }
        _ => Unit::Unknown,
    }
}

#[derive(Clone, Copy)]
enum Bus {
    System,
    Session,
}

/// Connexions gardées d'une vérification à l'autre (l'écran d'accueil
/// revérifie toutes les 3 s) ; oubliées après une erreur.
static SYSTEM_BUS: Mutex<Option<Connection>> = Mutex::new(None);
static SESSION_BUS: Mutex<Option<Connection>> = Mutex::new(None);

fn slot(bus: Bus) -> &'static Mutex<Option<Connection>> {
    match bus {
        Bus::System => &SYSTEM_BUS,
        Bus::Session => &SESSION_BUS,
    }
}

fn connection(bus: Bus) -> zbus::Result<Connection> {
    let mut slot = slot(bus).lock();
    if let Some(connection) = slot.as_ref() {
        return Ok(connection.clone());
    }
    let builder = match bus {
        Bus::System => zbus::blocking::connection::Builder::system()?,
        Bus::Session => zbus::blocking::connection::Builder::session()?,
    };
    let connection = builder.method_timeout(Duration::from_secs(2)).build()?;
    *slot = Some(connection.clone());
    Ok(connection)
}

fn read_unit(bus: Bus, name: &str) -> zbus::Result<Unit> {
    let result = query_unit(bus, name);
    if result.is_err() {
        slot(bus).lock().take();
    }
    result
}

fn query_unit(bus: Bus, name: &str) -> zbus::Result<Unit> {
    let connection = connection(bus)?;
    let manager: zbus::blocking::Proxy = Builder::new(&connection)
        .destination("org.freedesktop.systemd1")?
        .path("/org/freedesktop/systemd1")?
        .interface("org.freedesktop.systemd1.Manager")?
        .cache_properties(CacheProperties::No)
        .build()?;
    // LoadUnit lit l'unité sur le disque sans la démarrer.
    let path: zbus::zvariant::OwnedObjectPath = manager.call("LoadUnit", &(name,))?;
    let unit: zbus::blocking::Proxy = Builder::new(&connection)
        .destination("org.freedesktop.systemd1")?
        .path(path)?
        .interface("org.freedesktop.systemd1.Unit")?
        .cache_properties(CacheProperties::No)
        .build()?;
    let load_state: String = unit.get_property("LoadState")?;
    if load_state != "loaded" {
        return Ok(Unit::NotFound);
    }
    Ok(Unit::Loaded(Service {
        user: matches!(bus, Bus::Session),
        active_state: unit.get_property("ActiveState")?,
        sub_state: unit.get_property("SubState")?,
        unit_file_state: unit.get_property("UnitFileState")?,
    }))
}

/// Délai de `StartUnit` : le temps de saisir le mot de passe demandé par
/// polkit.
const START_TIMEOUT: Duration = Duration::from_secs(300);

/// Démarre `ollama.service` par D-Bus (`StartUnit`, mode `replace`) : unité
/// du système avec autorisation interactive (polkit demande le mot de passe
/// par l'agent de la session), ou de l'utilisateur sur le bus de session.
/// Bloquant : à appeler hors du runtime asynchrone.
pub fn start_unit(user: bool) -> StartResult {
    start_named(user, UNIT_NAME)
}

fn start_named(user: bool, name: &str) -> StartResult {
    let builder = if user {
        zbus::blocking::connection::Builder::session()
    } else {
        zbus::blocking::connection::Builder::system()
    };
    // Connexion à part : celle des lectures a un délai de 2 s.
    let result = builder
        .and_then(|builder| builder.method_timeout(START_TIMEOUT).build())
        .and_then(|connection| {
            let manager: zbus::blocking::Proxy = Builder::new(&connection)
                .destination("org.freedesktop.systemd1")?
                .path("/org/freedesktop/systemd1")?
                .interface("org.freedesktop.systemd1.Manager")?
                .cache_properties(CacheProperties::No)
                .build()?;
            manager.call_with_flags::<_, _, zbus::zvariant::OwnedObjectPath>(
                "StartUnit",
                zbus::proxy::MethodFlags::AllowInteractiveAuth.into(),
                &(name, "replace"),
            )
        });
    match result {
        Ok(_) => StartResult::Started,
        Err(zbus::Error::MethodError(name, detail, _)) => {
            start_error(name.as_str(), detail.unwrap_or_default())
        }
        Err(error) => StartResult::Failed(error.to_string()),
    }
}

/// Erreur D-Bus de `StartUnit`, par son nom.
pub(crate) fn start_error(name: &str, detail: String) -> StartResult {
    match name {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
        | "org.freedesktop.PolicyKit1.Error.NotAuthorized"
        | "org.freedesktop.PolicyKit1.Error.Cancelled" => StartResult::NotAuthorized,
        "org.freedesktop.systemd1.NoSuchUnit" | "org.freedesktop.systemd1.LoadFailed" => {
            StartResult::NoUnit
        }
        _ if detail.is_empty() => StartResult::Failed(name.to_owned()),
        _ => StartResult::Failed(detail),
    }
}

/// Réglages du service système `ollama.service` qui décident du dossier des
/// modèles (Settings > Model location), lus sans droits par D-Bus (interfaces
/// `Unit` et `Service`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceDetails {
    /// `ActiveState` : active, inactive, activating, failed...
    pub active_state: String,
    /// Processus principal ; 0 si le service ne tourne pas.
    pub main_pid: u32,
    /// `Environment=`, déjà fusionné par systemd (unité et drop-ins, dans
    /// l'ordre : la dernière affectation d'une variable l'emporte).
    pub environment: Vec<String>,
    /// Fichiers de `EnvironmentFile=` (ils l'emportent sur `Environment=`).
    pub environment_files: Vec<String>,
    /// `User=` ; vide : root.
    pub user: String,
    /// `Group=` ; vide : groupe principal de l'utilisateur.
    pub group: String,
    pub dynamic_user: bool,
    /// `ProtectHome=` : no, yes, read-only, tmpfs.
    pub protect_home: String,
    /// `ProtectSystem=` : no, yes, full, strict.
    pub protect_system: String,
    /// Drop-ins appliqués, dans l'ordre.
    pub drop_in_paths: Vec<String>,
    /// `BindPaths=` et `BindReadOnlyPaths=`, (source, destination) : dossiers
    /// que le service voit malgré `ProtectHome=tmpfs` (dossier `.ollama` de
    /// son `HOME`), si leur source existe.
    pub binds: Vec<(String, String)>,
}

/// Lit `ollama.service` sur le bus système ; `Ok(None)` si l'unité n'existe
/// pas. Bloquant : à appeler hors du runtime asynchrone.
pub fn system_service_details() -> zbus::Result<Option<ServiceDetails>> {
    let result = query_service_details(UNIT_NAME);
    if result.is_err() {
        slot(Bus::System).lock().take();
    }
    result
}

fn query_service_details(name: &str) -> zbus::Result<Option<ServiceDetails>> {
    let connection = connection(Bus::System)?;
    let manager: zbus::blocking::Proxy = Builder::new(&connection)
        .destination("org.freedesktop.systemd1")?
        .path("/org/freedesktop/systemd1")?
        .interface("org.freedesktop.systemd1.Manager")?
        .cache_properties(CacheProperties::No)
        .build()?;
    let path: zbus::zvariant::OwnedObjectPath = manager.call("LoadUnit", &(name,))?;
    let unit: zbus::blocking::Proxy = Builder::new(&connection)
        .destination("org.freedesktop.systemd1")?
        .path(path.clone())?
        .interface("org.freedesktop.systemd1.Unit")?
        .cache_properties(CacheProperties::No)
        .build()?;
    let load_state: String = unit.get_property("LoadState")?;
    if load_state != "loaded" {
        return Ok(None);
    }
    let service: zbus::blocking::Proxy = Builder::new(&connection)
        .destination("org.freedesktop.systemd1")?
        .path(path)?
        .interface("org.freedesktop.systemd1.Service")?
        .cache_properties(CacheProperties::No)
        .build()?;
    let environment_files: Vec<(String, bool)> = service.get_property("EnvironmentFiles")?;
    // (source, destination, ignorer l'absence, options de montage) ; une
    // lecture impossible ne prive pas du reste.
    let mut binds = Vec::new();
    for property in ["BindPaths", "BindReadOnlyPaths"] {
        let entries: Vec<(String, String, bool, u64)> =
            service.get_property(property).unwrap_or_default();
        binds.extend(entries.into_iter().map(|(source, destination, _, _)| {
            let destination = if destination.is_empty() {
                source.clone()
            } else {
                destination
            };
            (source, destination)
        }));
    }
    Ok(Some(ServiceDetails {
        active_state: unit.get_property("ActiveState")?,
        main_pid: service.get_property("MainPID")?,
        environment: service.get_property("Environment")?,
        environment_files: environment_files
            .into_iter()
            .map(|(file, _)| file)
            .collect(),
        user: service.get_property("User")?,
        group: service.get_property("Group")?,
        dynamic_user: service.get_property("DynamicUser")?,
        protect_home: service.get_property("ProtectHome")?,
        protect_system: service.get_property("ProtectSystem")?,
        drop_in_paths: unit.get_property("DropInPaths")?,
        binds,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "dépend de la machine : unité ollama.service présente"]
    fn reads_the_ollama_unit_of_this_machine() {
        let unit = ollama_unit();
        assert!(matches!(unit, Unit::Loaded(_)), "{unit:?}");
    }

    #[test]
    #[ignore = "dépend de la machine : unité ollama.service présente"]
    fn reads_the_service_details_of_this_machine() {
        let details = system_service_details().unwrap().unwrap();
        assert!(!details.environment.is_empty(), "{details:?}");
        assert!(!details.protect_home.is_empty(), "{details:?}");
    }

    /// L'appel `StartUnit` tel que ChatonLM l'envoie est compris par le vrai
    /// systemd. Bus de session et unité inexistante : ni polkit, ni invite,
    /// rien n'est démarré, et l'erreur de systemd est reconnue. À lancer à la
    /// main (comme les deux essais ci-dessus, hors de `check.sh`).
    #[test]
    #[ignore = "dépend de la machine : systemd de l'utilisateur sur le bus de session"]
    fn start_unit_is_understood_by_the_systemd_of_this_machine() {
        assert_eq!(
            start_named(true, "chatonlm-contract-absent.service"),
            StartResult::NoUnit
        );
    }

    #[test]
    fn start_errors_are_told_apart() {
        for name in [
            "org.freedesktop.DBus.Error.AccessDenied",
            "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
        ] {
            assert_eq!(
                start_error(name, "denied".into()),
                StartResult::NotAuthorized
            );
        }
        assert_eq!(
            start_error("org.freedesktop.systemd1.NoSuchUnit", String::new()),
            StartResult::NoUnit
        );
        assert_eq!(
            start_error("org.freedesktop.systemd1.JobFailed", "Unit failed".into()),
            StartResult::Failed("Unit failed".into())
        );
        assert_eq!(
            start_error("org.example.Other", String::new()),
            StartResult::Failed("org.example.Other".into())
        );
    }

    #[test]
    fn missing_unit_is_not_found() {
        // Sur une machine sans systemd, le bus est absent : Unknown est admis.
        for bus in [Bus::System, Bus::Session] {
            let unit = read_unit(bus, "chatonlm-does-not-exist.service").unwrap_or(Unit::Unknown);
            assert!(matches!(unit, Unit::NotFound | Unit::Unknown), "{unit:?}");
        }
    }
}
