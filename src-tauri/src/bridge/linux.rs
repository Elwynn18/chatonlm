//! Point d'accès du pont sous Linux : socket Unix dans
//! `$XDG_RUNTIME_DIR/<identifiant>/` (dossier 0700 à nous, socket 0600), pair
//! vérifié par `SO_PEERCRED` (même utilisateur seulement). Jamais de repli
//! dans `/tmp` ni de socket abstrait (non protégé par le système de fichiers).

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use super::{Io, OpenError};

const SOCKET: &str = "bridge.sock";

/// Longueur utile de `sun_path`.
const MAX_SOCKET_PATH: usize = 107;

/// Dossier du point d'accès et du fichier de découverte ; `None` sans
/// `$XDG_RUNTIME_DIR` (session sans systemd-logind).
pub fn discovery_dir(identifier: &str, _data_dir: &Path) -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    let runtime = PathBuf::from(runtime);
    runtime.is_absolute().then(|| runtime.join(identifier))
}

fn uid() -> u32 {
    // SAFETY: getuid n'échoue jamais et ne touche à aucune mémoire.
    unsafe { libc::getuid() }
}

/// Dossier à nous, 0700 : créé, ou vérifié s'il existe (pas un lien, pas à
/// un autre, droits resserrés).
fn own_dir(dir: &Path) -> Result<(), OpenError> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if !meta.is_dir() || meta.uid() != uid() {
                return Err(OpenError::Unsafe);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(OpenError::Io)?;
        }
        Err(error) => return Err(OpenError::Io(error)),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(OpenError::Io)
}

pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
    endpoint: String,
}

/// Ouvre le point d'accès dans `dir`. Un socket resté d'un arrêt brutal est
/// retiré ; un socket qui répond appartient à une autre instance.
pub async fn open(dir: &Path) -> Result<Listener, OpenError> {
    own_dir(dir)?;
    let path = dir.join(SOCKET);
    let endpoint = path.to_str().ok_or(OpenError::Unsafe)?.to_owned();
    if endpoint.len() > MAX_SOCKET_PATH {
        return Err(OpenError::PathTooLong);
    }
    if std::fs::symlink_metadata(&path).is_ok() {
        if UnixStream::connect(&path).await.is_ok() {
            return Err(OpenError::InUse);
        }
        std::fs::remove_file(&path).map_err(OpenError::Io)?;
    }
    let listener = UnixListener::bind(&path).map_err(OpenError::Io)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(OpenError::Io)?;
    Ok(Listener {
        listener,
        path,
        endpoint,
    })
}

impl Listener {
    /// Adresse donnée à l'extension (`net.connect({ path })`).
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Prochaine connexion d'un programme du même utilisateur ; les autres
    /// sont fermées aussitôt.
    pub async fn accept(&mut self) -> std::io::Result<Box<dyn Io>> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            match stream.peer_cred() {
                Ok(credentials) if credentials.uid() == uid() => return Ok(Box::new(stream)),
                _ => drop(stream),
            }
        }
    }

    /// Retire le socket.
    pub fn close(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_same_user_client_connects_and_a_second_server_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("bridge");
        let mut listener = open(&dir).await.unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let socket = std::fs::metadata(dir.join(SOCKET)).unwrap();
        assert_eq!(socket.permissions().mode() & 0o777, 0o600);

        let endpoint = listener.endpoint().to_owned();
        let client = tokio::spawn(async move {
            let mut stream = UnixStream::connect(endpoint).await.unwrap();
            stream.write_all(b"hi").await.unwrap();
        });
        let mut served = listener.accept().await.unwrap();
        let mut read = [0; 2];
        served.read_exact(&mut read).await.unwrap();
        assert_eq!(&read, b"hi");
        client.await.unwrap();

        assert!(matches!(open(&dir).await, Err(OpenError::InUse)));
        listener.close();
        assert!(!dir.join(SOCKET).exists());
    }

    #[tokio::test]
    async fn a_stale_socket_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("bridge");
        let first = open(&dir).await.unwrap();
        // Arrêt brutal : le fichier reste, personne n'écoute.
        drop(first);
        assert!(dir.join(SOCKET).exists());
        let second = open(&dir).await.unwrap();
        second.close();
    }

    #[tokio::test]
    async fn a_symlinked_or_foreign_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("bridge");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(open(&link).await, Err(OpenError::Unsafe)));
        let file = dir.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(matches!(open(&file).await, Err(OpenError::Unsafe)));
    }

    #[tokio::test]
    async fn a_too_long_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let long = dir.path().join("x".repeat(120));
        assert!(matches!(open(&long).await, Err(OpenError::PathTooLong)));
    }

    #[test]
    fn the_runtime_dir_is_required() {
        // Lecture seule de la variable : pas de modification de l'environnement
        // (essais en parallèle).
        let found = discovery_dir("com.example", Path::new("/data"));
        match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
            Some(runtime) => assert_eq!(found, Some(PathBuf::from(runtime).join("com.example"))),
            None => assert_eq!(found, None),
        }
    }
}
