//! Point d'accès du pont sous Windows : tube nommé au nom aléatoire, neuf à
//! chaque ouverture (les noms de tubes sont communs à toute la machine),
//! ouvert en première instance (`FILE_FLAG_FIRST_PIPE_INSTANCE` : personne
//! n'a pu le créer avant nous), refusant les clients distants, et protégé par
//! une liste d'accès qui ne donne des droits qu'à l'utilisateur courant (la
//! liste par défaut laisse lire Everyone et Anonymous). Le fichier de
//! découverte est dans le dossier des données, sous le profil de
//! l'utilisateur.

use std::path::{Path, PathBuf};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PWSTR};

use super::{Io, OpenError};

/// Dossier du fichier de découverte : celui des données de l'app.
pub fn discovery_dir(_identifier: &str, data_dir: &Path) -> Option<PathBuf> {
    Some(data_dir.to_path_buf())
}

fn other(error: windows::core::Error) -> std::io::Error {
    std::io::Error::other(error)
}

/// Descripteur de sécurité rendu par le système, libéré avec lui.
struct Descriptor(PSECURITY_DESCRIPTOR);

// SAFETY: le descripteur est un bloc de mémoire en lecture seule après sa
// création, libéré une seule fois (Drop) ; il ne dépend d'aucun fil.
unsafe impl Send for Descriptor {}
unsafe impl Sync for Descriptor {}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: bloc alloué par ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0.0)));
        }
    }
}

/// SID de l'utilisateur du processus, en texte (« S-1-5-21-… »).
fn user_sid() -> std::io::Result<String> {
    // SAFETY: appels Win32 documentés ; le jeton est fermé, la chaîne du SID
    // libérée ; le tampon est aligné pour TOKEN_USER (u64).
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(other)?;
        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let read = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        );
        let _ = CloseHandle(token);
        read.map_err(other)?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).map_err(other)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid.map_err(std::io::Error::other)
    }
}

fn descriptor(sddl: &str) -> std::io::Result<Descriptor> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: chaîne valide, sortie écrite par le système.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(other)?;
    }
    Ok(Descriptor(descriptor))
}

fn create(name: &str, descriptor: &Descriptor, first: bool) -> std::io::Result<NamedPipeServer> {
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.0,
        bInheritHandle: false.into(),
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // SAFETY: `attributes` pointe un SECURITY_ATTRIBUTES valide pendant
    // l'appel ; le descripteur vit aussi longtemps que le Listener.
    unsafe {
        options.create_with_security_attributes_raw(
            name,
            (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
        )
    }
}

pub struct Listener {
    name: String,
    descriptor: Descriptor,
    /// Instance qui attend le prochain client.
    next: NamedPipeServer,
}

/// Ouvre un tube neuf.
pub async fn open(_dir: &Path) -> Result<Listener, OpenError> {
    let name = super::pipe_name(&super::proof::random::<16>());
    let sid = user_sid().map_err(OpenError::Io)?;
    let descriptor = descriptor(&super::pipe_sddl(&sid)).map_err(OpenError::Io)?;
    let next = create(&name, &descriptor, true).map_err(OpenError::Io)?;
    Ok(Listener {
        name,
        descriptor,
        next,
    })
}

impl Listener {
    /// Adresse donnée à l'extension (`net.connect({ path })`).
    pub fn endpoint(&self) -> &str {
        &self.name
    }

    /// Prochaine connexion. L'instance suivante est créée avant de rendre
    /// celle-ci : le nom n'est jamais libre entre deux clients.
    pub async fn accept(&mut self) -> std::io::Result<Box<dyn Io>> {
        self.next.connect().await?;
        let fresh = create(&self.name, &self.descriptor, false)?;
        let connected = std::mem::replace(&mut self.next, fresh);
        Ok(Box::new(connected))
    }

    /// Le tube disparaît avec sa dernière instance.
    pub fn close(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    #[tokio::test]
    async fn a_client_connects_and_nobody_can_create_the_pipe_again_first() {
        let dir = std::env::temp_dir();
        let mut listener = open(&dir).await.unwrap();
        let endpoint = listener.endpoint().to_owned();
        assert!(endpoint.starts_with(r"\\.\pipe\chatonlm-"));

        let client_endpoint = endpoint.clone();
        let client = tokio::spawn(async move {
            let mut client = ClientOptions::new().open(&client_endpoint).unwrap();
            client.write_all(b"hi").await.unwrap();
        });
        let mut served = listener.accept().await.unwrap();
        let mut read = [0; 2];
        served.read_exact(&mut read).await.unwrap();
        assert_eq!(&read, b"hi");
        client.await.unwrap();

        // Un second serveur ne peut pas devenir « premier » sur ce nom.
        assert!(create(&endpoint, &listener.descriptor, true).is_err());
        // Un second client est servi par l'instance déjà prête.
        let second = ClientOptions::new().open(&endpoint);
        assert!(second.is_ok());
    }

    #[test]
    fn the_user_sid_is_found() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
        descriptor(&crate::bridge::pipe_sddl(&sid)).unwrap();
    }
}
