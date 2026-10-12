//! Outils des essais de la machine d'états : faux serveur de mises à jour
//! (HTTP local) et vrai greffon, construit dans une app simulée
//! (`tauri::test`). Paquet et signature servis par défaut : l'AppImage
//! d'essai de `staging::tests` (clé d'essai, version 9.9.9).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tauri_plugin_updater::{Updater, UpdaterExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::Engine;
use crate::updater::staging::tests::{APPIMAGE_SIGNATURE, TEST_PUBKEY, test_appimage};
use crate::updater::{UpdateState, UpdateStatus};

/// Clé de cette plateforme dans `latest.json` (« linux-x86_64 »,
/// « windows-x86_64 »), celle que cherche le greffon.
pub(super) fn platform_key() -> String {
    tauri_plugin_updater::target().expect("plateforme connue du greffon")
}

pub(super) const WAIT: Duration = Duration::from_secs(15);

/// Ce que sert le faux serveur.
pub(super) struct Served {
    /// `None` : 204 (aucune mise à jour).
    pub(super) manifest: Option<serde_json::Value>,
    /// Signature annoncée.
    pub(super) signature: String,
    /// Code de `/latest.json` (200 par défaut).
    pub(super) status: u16,
    pub(super) package: Vec<u8>,
    /// Attente avant de répondre à `/latest.json`, puis à `/package`.
    pub(super) delay: Duration,
    pub(super) package_delay: Duration,
}

pub(super) struct FakeServer {
    pub(super) base: String,
    pub(super) served: Arc<Mutex<Served>>,
    pub(super) requests: Arc<Mutex<Vec<String>>>,
}

impl FakeServer {
    pub(super) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(Mutex::new(Served {
            manifest: None,
            signature: APPIMAGE_SIGNATURE.into(),
            status: 200,
            package: test_appimage(),
            delay: Duration::ZERO,
            package_delay: Duration::ZERO,
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (state, log) = (Arc::clone(&served), Arc::clone(&requests));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (state, log) = (Arc::clone(&state), Arc::clone(&log));
                tokio::spawn(serve(stream, state, log));
            }
        });
        Self {
            base,
            served,
            requests,
        }
    }

    pub(super) fn endpoint(&self) -> String {
        format!("{}/latest.json", self.base)
    }

    /// Annonce `version`, paquet à `/package`, signature d'essai (celle de
    /// l'AppImage, sauf [`Self::serve`]).
    pub(super) fn announce(&self, version: &str) {
        let mut served = self.served.lock();
        served.manifest = Some(serde_json::json!({
            "version": version,
            "notes": "Faster answers.",
            "pub_date": "2026-10-07T12:00:00Z",
            "platforms": {
                platform_key(): {
                    "url": format!("{}/package", self.base),
                    "signature": served.signature,
                }
            }
        }));
    }

    /// Paquet et signature servis (puis annoncés par [`Self::announce`]).
    pub(super) fn serve(&self, package: Vec<u8>, signature: &str) {
        let mut served = self.served.lock();
        served.package = package;
        served.signature = signature.into();
    }

    pub(super) fn count(&self, path: &str) -> usize {
        self.requests.lock().iter().filter(|p| *p == path).count()
    }
}

pub(super) async fn serve(
    mut stream: tokio::net::TcpStream,
    served: Arc<Mutex<Served>>,
    requests: Arc<Mutex<Vec<String>>>,
) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
    requests.lock().push(path.clone());
    let (status, body, delay) = {
        let served = served.lock();
        match path.as_str() {
            "/latest.json" => match &served.manifest {
                Some(manifest) => (
                    served.status,
                    serde_json::to_vec(manifest).unwrap(),
                    served.delay,
                ),
                None => (204, Vec::new(), served.delay),
            },
            "/package" => (200, served.package.clone(), served.package_delay),
            _ => (404, Vec::new(), Duration::ZERO),
        }
    };
    tokio::time::sleep(delay).await;
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.shutdown().await;
}

/// Greffon de l'app simulée, vers le faux serveur ; `exe` : l'AppImage à
/// remplacer.
pub(super) fn plugin_updater(endpoint: &str, exe: &Path) -> Updater {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({ "pubkey": TEST_PUBKEY, "endpoints": [endpoint] }),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .expect("mock app");
    app.updater_builder()
        .executable_path(exe)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

/// Attend un état qui satisfait `predicate` (tous les états publiés sont
/// gardés dans `seen`).
pub(super) async fn wait_for(
    engine: &Engine,
    what: &str,
    predicate: impl Fn(&UpdateState) -> bool,
) -> UpdateStatus {
    let mut receiver = engine.subscribe();
    let found = tokio::time::timeout(WAIT, async {
        loop {
            let status = receiver.borrow_and_update().clone();
            if predicate(&status.state) {
                return status;
            }
            receiver.changed().await.unwrap();
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("{what} : état {:?}", engine.status().state))
}

pub(super) fn ready(version: &str) -> impl Fn(&UpdateState) -> bool {
    let version = version.to_owned();
    move |state| matches!(state, UpdateState::Ready { version: v, .. } if *v == version)
}

pub(super) fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
