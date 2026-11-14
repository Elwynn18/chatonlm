//! Faux GitHub « Releases » pour les outils téléchargés à la demande de
//! l'outil Transcription (yt-dlp et Deno ; module `tools` de l'app) : la
//! redirection de `releases/latest`, les archives onedir (fabriquées à la
//! volée), les sommes (`SHA2-256SUMS` signé par une clé d'essai OpenPGP v4
//! RSA/SHA-512 pour yt-dlp ; `.sha256sum` à côté de chaque archive pour
//! Deno, au format de Linux ou à celui de PowerShell pour l'archive
//! Windows, comme les vraies) et les requêtes `Range` (taille lue par
//! `bytes=0-0`, reprises).
//!
//! Routes (`<url>` = [`FakeGithub::url`]) :
//! - `GET /yt-dlp/yt-dlp/releases/latest` → 302 vers `…/releases/tag/<v>` ;
//!   `GET /yt-dlp/yt-dlp/releases/download/<v>/{yt-dlp_linux.zip,
//!   yt-dlp_win.zip, SHA2-256SUMS, SHA2-256SUMS.sig}` ;
//! - `GET /denoland/deno/releases/latest` → 302 vers `…/releases/tag/v<v>` ;
//!   `GET /denoland/deno/releases/download/v<v>/deno-x86_64-{unknown-linux-gnu,
//!   pc-windows-msvc}.zip[.sha256sum]` ;
//! - `POST /fake/publish {"tool": "yt-dlp"|"deno", "version": "…"}` : une
//!   nouvelle version devient la dernière (les anciennes restent servies) ;
//! - `POST /fake/fault {"tool": …, "fault": …}` : défaut de la dernière
//!   version ([`Fault`]) ;
//! - `GET /fake/requests` (méthode et chemin reçus), `GET /fake/pubkey`
//!   (clé publique d'essai, armure OpenPGP : `CHATONLM_TOOLS_PUBKEY` de
//!   l'app, builds de débogage et e2e seulement).
//!
//! Contenu des « binaires » : par défaut un script `sh` qui répond à
//! `--version` comme le vrai (la version publiée) ; un e2e donne les faux
//! binaires compilés ([`Binaries`] : `fake-yt-dlp`, `fake-deno`), et
//! l'archive porte alors aussi la version publiée dans un fichier
//! ([`crate::ytdlp::VERSION_FILE`]) que ces faux annoncent.

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use ring::signature::{KeyPair, RSA_PKCS1_SHA512, RsaKeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Dernières versions servies au départ (relevé du 14 nov. 2026).
pub const YTDLP_VERSION: &str = "2026.10.26";
pub const DENO_VERSION: &str = "2.9.8";

/// Clé d'essai qui signe les `SHA2-256SUMS` du faux (PKCS#8, RSA 2048).
const SIGNING_KEY: &[u8] = include_bytes!("../fixtures/github/signing-key.pk8");
/// Autre clé d'essai : une signature valide, mais pas de la bonne clé.
const OTHER_KEY: &[u8] = include_bytes!("../fixtures/github/other-key.pk8");
/// Date de création des clés et des signatures d'essai (fixe).
const CREATED: u32 = 1_700_000_000;

/// Outil d'une Release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    YtDlp,
    Deno,
}

impl Tool {
    fn parse(name: &str) -> Option<Tool> {
        match name {
            "yt-dlp" => Some(Tool::YtDlp),
            "deno" => Some(Tool::Deno),
            _ => None,
        }
    }

    /// Chemin du dépôt sous l'adresse du faux.
    pub fn repo(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp/yt-dlp",
            Tool::Deno => "denoland/deno",
        }
    }

    /// Étiquette d'une version (`v2.9.8` pour Deno).
    pub fn tag(self, version: &str) -> String {
        match self {
            Tool::YtDlp => version.to_owned(),
            Tool::Deno => format!("v{version}"),
        }
    }
}

/// Défaut servi pour la dernière version d'un outil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fault {
    /// Somme de l'archive fausse (le téléchargement échoue à la vérification).
    BadSum,
    /// Signature de `SHA2-256SUMS` abîmée (un octet changé).
    BadSignature,
    /// Signature valide, faite par une autre clé.
    OtherKey,
    /// Archive tronquée, somme de l'archive tronquée (illisible à
    /// l'extraction).
    Truncated,
    /// Entrée `../evil.txt` dans l'archive.
    ZipSlip,
    /// Lien symbolique dans l'archive.
    Symlink,
    /// Archive absente de la Release (nom changé).
    MissingAsset,
    /// `releases/latest` répond 404 (pas de redirection).
    NoLatest,
}

impl Fault {
    fn parse(name: &str) -> Option<Fault> {
        Some(match name {
            "bad-sum" => Fault::BadSum,
            "bad-signature" => Fault::BadSignature,
            "other-key" => Fault::OtherKey,
            "truncated" => Fault::Truncated,
            "zip-slip" => Fault::ZipSlip,
            "symlink" => Fault::Symlink,
            "missing-asset" => Fault::MissingAsset,
            "no-latest" => Fault::NoLatest,
            _ => return None,
        })
    }
}

/// Contenu des binaires des archives ; `None` : script `sh` qui répond à
/// `--version`.
#[derive(Clone, Debug, Default)]
pub struct Binaries {
    pub yt_dlp: Option<Vec<u8>>,
    pub deno: Option<Vec<u8>>,
}

/// Requête reçue (hors `/fake/…`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubRequest {
    pub method: String,
    pub path: String,
    pub range: Option<String>,
}

/// Archive fabriquée : outil, version, nom, défaut.
type ArchiveKey = (Tool, String, String, Option<Fault>);

struct Shared {
    binaries: Binaries,
    /// Versions publiées de chaque outil, la dernière à la fin.
    versions: Mutex<HashMap<Tool, Vec<String>>>,
    faults: Mutex<HashMap<Tool, Fault>>,
    requests: Mutex<Vec<GithubRequest>>,
    /// Archives déjà fabriquées (outil, version, nom, défaut).
    archives: Mutex<HashMap<ArchiveKey, Arc<Vec<u8>>>>,
}

pub struct FakeGithub {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

impl FakeGithub {
    /// Démarre le serveur sur un port libre de 127.0.0.1.
    pub async fn start() -> std::io::Result<Self> {
        Self::start_with(SocketAddr::from(([127, 0, 0, 1], 0)), Binaries::default()).await
    }

    pub async fn start_with(addr: SocketAddr, binaries: Binaries) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            binaries,
            versions: Mutex::new(HashMap::from([
                (Tool::YtDlp, vec![YTDLP_VERSION.to_owned()]),
                (Tool::Deno, vec![DENO_VERSION.to_owned()]),
            ])),
            faults: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            archives: Mutex::new(HashMap::new()),
        });
        let (shutdown, stop) = oneshot::channel::<()>();
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            shutdown: Some(shutdown),
            task,
            shared,
        })
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Adresse du dépôt de `tool` (`<url>/yt-dlp/yt-dlp`).
    pub fn repo_url(&self, tool: Tool) -> String {
        format!("{}/{}", self.url(), tool.repo())
    }

    /// `version` devient la dernière de `tool`.
    pub fn publish(&self, tool: Tool, version: &str) {
        publish(&self.shared, tool, version);
    }

    /// Défaut de la dernière version de `tool` (`None` : aucun).
    pub fn set_fault(&self, tool: Tool, fault: Option<Fault>) {
        let mut faults = self.shared.faults.lock().unwrap();
        match fault {
            Some(fault) => faults.insert(tool, fault),
            None => faults.remove(&tool),
        };
    }

    pub fn requests(&self) -> Vec<GithubRequest> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// Arrête le serveur (1 s au plus).
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

impl Drop for FakeGithub {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

fn publish(shared: &Shared, tool: Tool, version: &str) {
    let mut versions = shared.versions.lock().unwrap();
    let list = versions.entry(tool).or_default();
    list.retain(|known| known != version);
    list.push(version.to_owned());
}

/// Noms des archives d'un outil (Linux, Windows).
pub fn assets(tool: Tool) -> [&'static str; 2] {
    match tool {
        Tool::YtDlp => ["yt-dlp_linux.zip", "yt-dlp_win.zip"],
        Tool::Deno => [
            "deno-x86_64-unknown-linux-gnu.zip",
            "deno-x86_64-pc-windows-msvc.zip",
        ],
    }
}

/// Script `sh` qui imite `--version` de l'outil.
pub fn version_script(tool: Tool, version: &str) -> Vec<u8> {
    match tool {
        Tool::YtDlp => format!("#!/bin/sh\necho {version}\n").into_bytes(),
        Tool::Deno => format!(
            "#!/bin/sh\necho 'deno {version} (stable, release, x86_64-unknown-linux-gnu)'\n\
             echo 'v8 14.7.173.20-rusty'\necho 'typescript 5.9.2'\n"
        )
        .into_bytes(),
    }
}

/// Archive `name` de `tool` à `version`, avec le défaut `fault`.
pub fn archive(
    tool: Tool,
    version: &str,
    name: &str,
    binaries: &Binaries,
    fault: Option<Fault>,
) -> Vec<u8> {
    let windows = name.contains("win");
    let binary = match (tool, windows) {
        (Tool::YtDlp, false) => "yt-dlp_linux",
        (Tool::YtDlp, true) => "yt-dlp.exe",
        (Tool::Deno, false) => "deno",
        (Tool::Deno, true) => "deno.exe",
    };
    let given = match tool {
        Tool::YtDlp => binaries.yt_dlp.clone(),
        Tool::Deno => binaries.deno.clone(),
    };
    let binary_given = given.is_some();
    let content = given.unwrap_or_else(|| version_script(tool, version));
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let file = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let executable = file.unix_permissions(0o755);
    if tool == Tool::YtDlp {
        zip.add_directory("_internal/", file).unwrap();
        zip.start_file("_internal/THIRD_PARTY_LICENSES.txt", file)
            .unwrap();
        zip.write_all(b"Python: PSF-2.0\nmutagen: GPL-2.0-or-later\n")
            .unwrap();
        zip.add_directory("_internal/yt_dlp_ejs/", file).unwrap();
        zip.start_file("_internal/yt_dlp_ejs/solver.js", file)
            .unwrap();
        zip.write_all(b"// EJS\n").unwrap();
        zip.start_file("_internal/libpython3.14.so.1.0", executable)
            .unwrap();
        zip.write_all(&[0x7f, b'E', b'L', b'F']).unwrap();
    }
    zip.start_file(binary, executable).unwrap();
    zip.write_all(&content).unwrap();
    // Faux binaire compilé (e2e) : la version publiée, qu'il annonce à
    // `--version` (module `ytdlp`).
    if binary_given {
        let version_file = match tool {
            Tool::YtDlp => format!("_internal/{}", crate::ytdlp::VERSION_FILE),
            Tool::Deno => crate::ytdlp::VERSION_FILE.to_owned(),
        };
        zip.start_file(version_file, file).unwrap();
        zip.write_all(version.as_bytes()).unwrap();
    }
    match fault {
        Some(Fault::ZipSlip) => {
            zip.start_file("../evil.txt", file).unwrap();
            zip.write_all(b"evil").unwrap();
        }
        Some(Fault::Symlink) => {
            zip.add_symlink("_internal/escape", "/etc/passwd", file)
                .unwrap();
        }
        _ => {}
    }
    let mut bytes = zip.finish().unwrap().into_inner();
    if fault == Some(Fault::Truncated) {
        bytes.truncate(bytes.len() / 2);
    }
    bytes
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// MPI OpenPGP (nombre de bits sur 16 bits, octets sans zéro de tête).
fn mpi(bytes: &[u8]) -> Vec<u8> {
    let start = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    let bits = if bytes.is_empty() {
        0
    } else {
        (bytes.len() - 1) * 8 + (8 - bytes[0].leading_zeros() as usize)
    };
    let mut out = (bits as u16).to_be_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

/// En-tête de paquet au format nouveau (longueur sur 1, 2 ou 5 octets).
fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xC0 | tag];
    let len = body.len();
    if len < 192 {
        out.push(len as u8);
    } else if len < 8384 {
        let value = len - 192;
        out.push(((value >> 8) + 192) as u8);
        out.push((value & 0xff) as u8);
    } else {
        out.push(255);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(body);
    out
}

/// Module et exposant (DER `RSAPublicKey` : SEQUENCE { INTEGER, INTEGER }).
fn rsa_components(der: &[u8]) -> (Vec<u8>, Vec<u8>) {
    fn read(input: &[u8]) -> (u8, &[u8], &[u8]) {
        let tag = input[0];
        let (len, start) = if input[1] < 0x80 {
            (input[1] as usize, 2)
        } else {
            let count = (input[1] & 0x7f) as usize;
            let len = input[2..2 + count]
                .iter()
                .fold(0usize, |len, byte| (len << 8) | *byte as usize);
            (len, 2 + count)
        };
        (tag, &input[start..start + len], &input[start + len..])
    }
    let (_, sequence, _) = read(der);
    let (_, n, rest) = read(sequence);
    let (_, e, _) = read(rest);
    (n.to_vec(), e.to_vec())
}

/// Clé d'essai : paire RSA et corps du paquet de clé publique v4.
struct TestKey {
    pair: RsaKeyPair,
    body: Vec<u8>,
    fingerprint: [u8; 20],
}

impl TestKey {
    fn load(pkcs8: &[u8]) -> TestKey {
        let pair = RsaKeyPair::from_pkcs8(pkcs8).expect("clé d'essai valide");
        let (n, e) = rsa_components(pair.public_key().as_ref());
        let mut body = vec![4];
        body.extend_from_slice(&CREATED.to_be_bytes());
        body.push(1);
        body.extend(mpi(&n));
        body.extend(mpi(&e));
        let mut hashed = vec![0x99];
        hashed.extend_from_slice(&(body.len() as u16).to_be_bytes());
        hashed.extend_from_slice(&body);
        let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &hashed);
        let mut fingerprint = [0u8; 20];
        fingerprint.copy_from_slice(digest.as_ref());
        TestKey {
            pair,
            body,
            fingerprint,
        }
    }

    /// Signature détachée OpenPGP v4 (binaire, sur des données binaires),
    /// RSA PKCS#1 v1.5 / SHA-512.
    fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.sign_with(data, SignOptions::default())
    }

    fn sign_with(&self, data: &[u8], options: SignOptions) -> Vec<u8> {
        let mut hashed_subpackets = vec![22, 33, 4];
        hashed_subpackets.extend_from_slice(&self.fingerprint);
        hashed_subpackets.extend_from_slice(&[5, 2]);
        hashed_subpackets.extend_from_slice(&CREATED.to_be_bytes());
        if let Some(seconds) = options.expires_after {
            hashed_subpackets.extend_from_slice(&[5, 3]);
            hashed_subpackets.extend_from_slice(&seconds.to_be_bytes());
        }
        if options.critical_unknown {
            // Sous-paquet 100 (expérimental), marqué critique.
            hashed_subpackets.extend_from_slice(&[2, 0x80 | 100, 0]);
        }
        let kind = if options.text { 0x01 } else { 0x00 };
        let mut prefix = vec![4, kind, 1, 10];
        prefix.extend_from_slice(&(hashed_subpackets.len() as u16).to_be_bytes());
        prefix.extend_from_slice(&hashed_subpackets);
        let mut message = data.to_vec();
        message.extend_from_slice(&prefix);
        message.extend_from_slice(&[4, 0xff]);
        message.extend_from_slice(&(prefix.len() as u32).to_be_bytes());
        let digest = ring::digest::digest(&ring::digest::SHA512, &message);
        let mut signature = vec![0u8; self.pair.public().modulus_len()];
        self.pair
            .sign(
                &RSA_PKCS1_SHA512,
                &ring::rand::SystemRandom::new(),
                &message,
                &mut signature,
            )
            .expect("signature d'essai");
        let mut body = prefix;
        let mut unhashed = vec![9, 16];
        unhashed.extend_from_slice(&self.fingerprint[12..]);
        body.extend_from_slice(&(unhashed.len() as u16).to_be_bytes());
        body.extend_from_slice(&unhashed);
        body.extend_from_slice(&digest.as_ref()[..2]);
        body.extend(mpi(&signature));
        packet(2, &body)
    }

    /// Clé publique en armure OpenPGP (paquet de clé seul).
    fn armored(&self) -> String {
        armor("PGP PUBLIC KEY BLOCK", &packet(6, &self.body))
    }
}

static KEY: LazyLock<TestKey> = LazyLock::new(|| TestKey::load(SIGNING_KEY));
static OTHER: LazyLock<TestKey> = LazyLock::new(|| TestKey::load(OTHER_KEY));

/// CRC-24 de l'armure OpenPGP (RFC 4880 § 6.1).
fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0x00B7_04CE;
    for byte in data {
        crc ^= u32::from(*byte) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x0100_0000 != 0 {
                crc ^= 0x0186_4CFB;
            }
        }
    }
    crc & 0x00FF_FFFF
}

/// Armure OpenPGP de `data` (`kind` : « PGP SIGNATURE »…).
pub fn armor(kind: &str, data: &[u8]) -> String {
    let engine = base64::engine::general_purpose::STANDARD;
    let body = engine.encode(data);
    let mut out = format!("-----BEGIN {kind}-----\n\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap());
        out.push('\n');
    }
    out.push('=');
    out.push_str(&engine.encode(&crc24(data).to_be_bytes()[1..]));
    out.push_str(&format!("\n-----END {kind}-----\n"));
    out
}

/// Clé publique d'essai (armure) qui vérifie les signatures du faux.
pub fn test_public_key() -> String {
    KEY.armored()
}

/// Empreinte de la clé d'essai, en hexadécimal majuscule.
pub fn test_fingerprint() -> String {
    KEY.fingerprint.iter().map(|b| format!("{b:02X}")).collect()
}

/// Signature détachée de `data` par la clé d'essai.
pub fn sign(data: &[u8]) -> Vec<u8> {
    KEY.sign(data)
}

/// Variantes d'une signature d'essai (refus à essayer).
#[derive(Clone, Copy, Debug, Default)]
pub struct SignOptions {
    /// Expire ce nombre de secondes après sa création (2023-11-14).
    pub expires_after: Option<u32>,
    /// Sous-paquet critique inconnu.
    pub critical_unknown: bool,
    /// Signature de texte (type 0x01) et non de données binaires.
    pub text: bool,
}

/// Signature détachée de `data` par la clé d'essai, avec ces variantes.
pub fn sign_with_options(data: &[u8], options: SignOptions) -> Vec<u8> {
    KEY.sign_with(data, options)
}

/// Signature détachée de `data` par l'autre clé d'essai.
pub fn sign_with_other_key(data: &[u8]) -> Vec<u8> {
    OTHER.sign(data)
}

/// Clé publique de l'autre clé d'essai (armure).
pub fn other_public_key() -> String {
    OTHER.armored()
}

fn latest(shared: &Shared, tool: Tool) -> String {
    shared.versions.lock().unwrap()[&tool]
        .last()
        .cloned()
        .unwrap_or_default()
}

/// Défaut de `tool` à `version` (la dernière seulement).
fn fault_of(shared: &Shared, tool: Tool, version: &str) -> Option<Fault> {
    if latest(shared, tool) != version {
        return None;
    }
    shared.faults.lock().unwrap().get(&tool).copied()
}

fn archive_of(shared: &Shared, tool: Tool, version: &str, name: &str) -> Arc<Vec<u8>> {
    let fault = fault_of(shared, tool, version)
        .filter(|fault| matches!(fault, Fault::Truncated | Fault::ZipSlip | Fault::Symlink));
    let key = (tool, version.to_owned(), name.to_owned(), fault);
    shared
        .archives
        .lock()
        .unwrap()
        .entry(key)
        .or_insert_with(|| Arc::new(archive(tool, version, name, &shared.binaries, fault)))
        .clone()
}

/// `SHA2-256SUMS` de yt-dlp à `version`.
fn ytdlp_sums(shared: &Shared, version: &str) -> String {
    let fault = fault_of(shared, Tool::YtDlp, version);
    let mut out = String::new();
    for name in assets(Tool::YtDlp) {
        let hash = if fault == Some(Fault::BadSum) {
            "0".repeat(64)
        } else {
            sha256_hex(&archive_of(shared, Tool::YtDlp, version, name))
        };
        out.push_str(&format!("{hash}  {name}\n"));
        // Une ligne voisine au nom proche : le lecteur prend le nom exact.
        out.push_str(&format!("{}  {name}.bak\n", "1".repeat(64)));
    }
    out
}

/// `.sha256sum` de Deno : format de Linux, ou de PowerShell pour l'archive
/// Windows (comme les vraies Releases).
fn deno_sum(shared: &Shared, version: &str, name: &str) -> String {
    let hash = if fault_of(shared, Tool::Deno, version) == Some(Fault::BadSum) {
        "0".repeat(64)
    } else {
        sha256_hex(&archive_of(shared, Tool::Deno, version, name))
    };
    if name.contains("windows") {
        format!(
            "\r\nAlgorithm : SHA256\r\nHash      : {}\r\nPath      : D:\\a\\deno\\deno\\target\\release\\{name}\r\n\r\n",
            hash.to_ascii_uppercase()
        )
    } else {
        format!("{hash}  {name}\n")
    }
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_owned();
    if let Some(rest) = path.strip_prefix("/fake/") {
        return fake_route(&shared, &method, rest, &body);
    }
    shared.requests.lock().unwrap().push(GithubRequest {
        method: method.to_string(),
        path: path.clone(),
        range: headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });
    for tool in [Tool::YtDlp, Tool::Deno] {
        let Some(rest) = path
            .strip_prefix('/')
            .and_then(|path| path.strip_prefix(tool.repo()))
        else {
            continue;
        };
        if rest == "/releases/latest" {
            let version = latest(&shared, tool);
            if fault_of(&shared, tool, &version) == Some(Fault::NoLatest) {
                return StatusCode::NOT_FOUND.into_response();
            }
            let location = format!(
                "https://github.com/{}/releases/tag/{}",
                tool.repo(),
                tool.tag(&version)
            );
            return (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
        }
        if let Some(file) = rest.strip_prefix("/releases/download/") {
            return download(&shared, tool, file, &headers);
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn download(shared: &Shared, tool: Tool, file: &str, headers: &HeaderMap) -> Response {
    let Some((tag, name)) = file.split_once('/') else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let version = match tool {
        Tool::YtDlp => tag,
        Tool::Deno => tag.strip_prefix('v').unwrap_or(""),
    };
    if !shared.versions.lock().unwrap()[&tool]
        .iter()
        .any(|known| known == version)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let fault = fault_of(shared, tool, version);
    let body: Arc<Vec<u8>> = match (tool, name) {
        (Tool::YtDlp, "SHA2-256SUMS") => Arc::new(ytdlp_sums(shared, version).into_bytes()),
        (Tool::YtDlp, "SHA2-256SUMS.sig") => {
            let sums = ytdlp_sums(shared, version);
            let mut signature = match fault {
                Some(Fault::OtherKey) => OTHER.sign(sums.as_bytes()),
                _ => KEY.sign(sums.as_bytes()),
            };
            if fault == Some(Fault::BadSignature) {
                let last = signature.len() - 1;
                signature[last] ^= 0x01;
            }
            Arc::new(signature)
        }
        _ if fault == Some(Fault::MissingAsset) && assets(tool).contains(&name) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        _ if assets(tool).contains(&name) => archive_of(shared, tool, version, name),
        (Tool::Deno, sum) if sum.ends_with(".sha256sum") => {
            let archive = sum.trim_end_matches(".sha256sum");
            if !assets(tool).contains(&archive) {
                return StatusCode::NOT_FOUND.into_response();
            }
            Arc::new(deno_sum(shared, version, archive).into_bytes())
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ranged(&body, headers)
}

/// Réponse entière, ou la plage `bytes=a-[b]` demandée (206).
fn ranged(body: &[u8], headers: &HeaderMap) -> Response {
    let total = body.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| {
            let (start, end) = value.split_once('-')?;
            let start: usize = start.trim().parse().ok()?;
            let end = match end.trim() {
                "" => total.checked_sub(1)?,
                end => end.parse::<usize>().ok()?.min(total.checked_sub(1)?),
            };
            (start <= end).then_some((start, end))
        });
    match range {
        Some((start, end)) => (
            StatusCode::PARTIAL_CONTENT,
            [
                (
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                ),
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            ],
            body[start..=end].to_vec(),
        )
            .into_response(),
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            body.to_vec(),
        )
            .into_response(),
    }
}

fn fake_route(shared: &Shared, method: &Method, rest: &str, body: &[u8]) -> Response {
    let json_body = || serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    match (method.as_str(), rest) {
        ("GET", "requests") => {
            let requests: Vec<Value> = shared
                .requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| {
                    json!({ "method": request.method, "path": request.path, "range": request.range })
                })
                .collect();
            axum::Json(Value::Array(requests)).into_response()
        }
        ("GET", "pubkey") => test_public_key().into_response(),
        ("POST", "publish") => {
            let value = json_body();
            match (
                value["tool"].as_str().and_then(Tool::parse),
                value["version"].as_str(),
            ) {
                (Some(tool), Some(version)) if !version.is_empty() => {
                    publish(shared, tool, version);
                    StatusCode::NO_CONTENT.into_response()
                }
                _ => StatusCode::BAD_REQUEST.into_response(),
            }
        }
        ("POST", "fault") => {
            let value = json_body();
            let Some(tool) = value["tool"].as_str().and_then(Tool::parse) else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let mut faults = shared.faults.lock().unwrap();
            match value["fault"].as_str() {
                None | Some("none") => {
                    faults.remove(&tool);
                }
                Some(name) => match Fault::parse(name) {
                    Some(fault) => {
                        faults.insert(tool, fault);
                    }
                    None => return StatusCode::BAD_REQUEST.into_response(),
                },
            }
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc24_of_the_armor_matches_rfc_4880() {
        // Valeur de référence : CRC-24 de « 123456789 ».
        assert_eq!(crc24(b"123456789"), 0x21CF02);
    }

    #[test]
    fn archives_are_valid_zips_except_the_truncated_one() {
        let binaries = Binaries::default();
        let good = archive(Tool::YtDlp, "1", "yt-dlp_linux.zip", &binaries, None);
        let zip = zip::ZipArchive::new(Cursor::new(good)).unwrap();
        assert!(
            zip.file_names()
                .any(|name| name.is_ok_and(|name| name == "yt-dlp_linux"))
        );
        let truncated = archive(
            Tool::Deno,
            "2.9.8",
            "deno-x86_64-unknown-linux-gnu.zip",
            &binaries,
            Some(Fault::Truncated),
        );
        assert!(zip::ZipArchive::new(Cursor::new(truncated)).is_err());
    }
}
