//! Whisper sur la carte graphique (propre à ChatonLM, choix du 20 nov. 2026 :
//! « Auto partout » pour la dictée, la transcription et le mode vocal ;
//! recherche `.tools/research/108-ressources-research.md`).
//!
//! - Calcul dans un processus à part, `whisper-server` de whisper.cpp livré
//!   avec l'app (Vulkan, moteurs de calcul chargés à l'exécution), comme
//!   `llama-server` et `sd-server` : l'app elle-même ne lie jamais Vulkan
//!   (elle démarrerait mal sans chargeur), et arrêter le processus rend toute
//!   sa VRAM.
//! - Seulement si l'arbitre de la carte ([`Budget`]) trouve la place (au
//!   besoin en faisant rendre la VRAM aux calculs inactifs) et si le serveur
//!   charge vraiment le modèle sur une carte (journal) ; sinon le Whisper de
//!   l'app, sur le processeur, comme avant.
//! - Un serveur qui échoue en cours de route (arrêté, réponse illisible) est
//!   arrêté, et la suite passe sur le processeur ([`ServerWhisper`]).
//! - Mêmes réglages de décodage que transcribe-rs (recherche en faisceau de
//!   3, jetons non vocaux supprimés, seuil de silence 0,2) : mêmes textes
//!   sur la carte que sur le processeur.
//! - Paquets : `<ressources>/whisper-server/` (Linux `/usr/lib/chatonlm`,
//!   AppImage par un lien vers `usr/libexec/chatonlm`, Windows
//!   `<installation>\whisper-server\`) ; builds de débogage : à défaut,
//!   `.tools/runtimes/whisper-server/` (`scripts/build-whisper-server.sh`) ;
//!   builds de débogage et d'essai : `CHATONLM_WHISPER_SERVER_BIN`. Chargeur
//!   Vulkan de secours : celui de `llama-server/fallback/`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;
use tokio::sync::oneshot;

use super::audio::RATE;
use super::engine::{ModelSpec, Piece, Recognizer, Request, Timed, transcription_failed};
use crate::process::{
    Spawner, desktop_changes, is_executable, isolate, pick_port, rotate_log, scrub, spawn_with,
    terminate,
};

/// Dossier du serveur, sous les ressources comme dans la construction locale.
pub const DIR: &str = "whisper-server";

/// Variable des builds de débogage et d'essai qui donne le binaire (chemin
/// absolu ; vide : aucun).
pub const BIN_ENV: &str = "CHATONLM_WHISPER_SERVER_BIN";

/// Attente au plus du chargement du modèle par le serveur.
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// Intervalle entre deux regards à `/health` pendant le chargement.
const READY_POLL: Duration = Duration::from_millis(100);

/// Attente au plus d'une transcription (45 s de son au plus par requête :
/// moins d'une seconde sur une carte ; large pour une petite carte).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Fils du processeur du serveur (préparation du son, décodage : le gros du
/// calcul est sur la carte).
const SERVER_THREADS: usize = 4;

/// VRAM demandée pour un modèle de `bytes` octets : poids, plus tampons de
/// calcul (mesuré le 20 nov. 2026 : 844 Mio pour large-v3 turbo q5_0, fichier
/// de 574 Mo ; ×1,5 + 256 Mio laisse de la marge aux petits modèles).
pub fn vram_needed(bytes: u64) -> u64 {
    bytes
        .saturating_mul(3)
        .div_ceil(2)
        .saturating_add(256 * 1024 * 1024)
}

/// Arbitre de la carte graphique vu par Whisper (appels bloquants, depuis le
/// fil de chargement du modèle).
pub trait Budget: Send + Sync {
    /// Place pour `bytes` octets sur la carte, après avoir fait rendre la
    /// VRAM aux calculs inactifs s'il le faut : le moteur d'images, et les
    /// modèles de texte seulement pour une transcription longue (`long_job`) ;
    /// jamais au détriment d'un calcul en cours. Faux : pas de carte, ou pas
    /// assez de place.
    fn make_room(&self, bytes: u64, long_job: bool) -> bool;
    /// Whisper occupe la carte (`pid` du serveur, `bytes` demandés).
    fn taken(&self, pid: u32, bytes: u64);
    /// Whisper vient de rendre la carte (serveur arrêté).
    fn released(&self);
}

/// Ce qu'il faut pour lancer `whisper-server` (posé au démarrage de l'app).
pub struct Placement {
    pub bin: PathBuf,
    /// Dossier du chargeur Vulkan de secours (`llama-server/fallback`).
    pub fallback: Option<PathBuf>,
    /// Journal du serveur (le précédent gardé en `.1`).
    pub log: PathBuf,
    pub budget: Arc<dyn Budget>,
    /// Fil qui crée le processus (voir [`Spawner`]).
    spawner: Mutex<Option<Spawner>>,
}

impl Placement {
    pub fn new(
        bin: PathBuf,
        fallback: Option<PathBuf>,
        log: PathBuf,
        budget: Arc<dyn Budget>,
    ) -> Self {
        Self {
            bin,
            fallback,
            log,
            budget,
            spawner: Mutex::new(None),
        }
    }
}

/// Binaire de `whisper-server` : `overridden` (valeur de [`BIN_ENV`], si
/// elle est permise) l'emporte (absolu et exécutable, sinon aucun ; vide :
/// aucun), puis celui des ressources, puis celui de `dev_dir`.
pub fn locate(
    overridden: Option<&std::ffi::OsStr>,
    resource_dir: Option<&Path>,
    dev_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = overridden {
        let path = PathBuf::from(path);
        return (path.is_absolute() && is_executable(&path)).then_some(path);
    }
    let name = format!("whisper-server{}", std::env::consts::EXE_SUFFIX);
    [
        resource_dir.map(|dir| dir.join(DIR)),
        dev_dir.map(Path::to_path_buf),
    ]
    .into_iter()
    .flatten()
    .map(|dir| dir.join(&name))
    .find(|path| is_executable(path))
}

/// Construction locale (`.tools/runtimes/whisper-server`), dans les builds de
/// débogage seulement.
pub fn dev_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(".tools")
                .join("runtimes")
                .join(DIR),
        )
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Valeur de [`BIN_ENV`], dans les builds de débogage et d'essai seulement.
pub fn overridden() -> Option<OsString> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    {
        std::env::var_os(BIN_ENV)
    }
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    {
        None
    }
}

/// Serveur lancé pour un modèle.
pub struct Server {
    pid: u32,
    port: u16,
    exited: oneshot::Receiver<Option<ExitStatus>>,
}

impl Server {
    fn alive(&mut self) -> bool {
        matches!(
            self.exited.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        )
    }

    /// Arrêt borné, attendu : depuis un fil qui peut bloquer (fil de
    /// chargement, `spawn_blocking`), jamais depuis une tâche asynchrone.
    /// La VRAM est rendue au retour.
    fn stop_now(self) {
        let stopping = self.stopping();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(stopping),
            Err(_) => tauri::async_runtime::block_on(stopping),
        }
    }

    /// Arrêt borné, puis `then` (VRAM rendue).
    async fn stopping(self) {
        let Server {
            pid, mut exited, ..
        } = self;
        terminate(
            pid,
            &mut exited,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await;
    }
}

/// Lance le serveur pour `spec` s'il y a la place sur la carte et que le
/// modèle s'y charge vraiment. `Ok(None)` : pas de carte ou pas de place
/// (rien n'a été lancé, ou le serveur ne calculait que sur le processeur) ;
/// `Err` : le lancement a raté (détail pour le journal).
pub fn start(
    placement: &Placement,
    spec: &ModelSpec,
    long_job: bool,
) -> Result<Option<Server>, String> {
    let bytes = std::fs::metadata(&spec.file)
        .map_err(|error| error.to_string())?
        .len();
    let need = vram_needed(bytes);
    if !placement.budget.make_room(need, long_job) {
        return Ok(None);
    }
    let port = pick_port(None).map_err(|error| error.to_string())?;
    let command = prepare(placement, spec, port).map_err(|error| error.to_string())?;
    let child =
        tauri::async_runtime::block_on(spawn_with(&placement.spawner, "chatonlm-whisper", command))
            .map_err(|error| error.to_string())?;
    let pid = child.id();
    let mut server = Server {
        pid,
        port,
        exited: crate::process::watch_exit(child),
    };
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if !server.alive() {
            return Err(
                last_error(&placement.log).unwrap_or_else(|| "stopped while loading".into())
            );
        }
        if health(port) {
            break;
        }
        if Instant::now() >= deadline {
            server.stop_now();
            return Err("not ready in time".into());
        }
        std::thread::sleep(READY_POLL);
    }
    if !on_card(&read_log(&placement.log)) {
        // Aucune carte vue par Vulkan : le Whisper de l'app fait aussi bien.
        server.stop_now();
        return Ok(None);
    }
    placement.budget.taken(pid, need);
    Ok(Some(server))
}

/// Commande du serveur et son journal.
fn prepare(placement: &Placement, spec: &ModelSpec, port: u16) -> std::io::Result<Command> {
    rotate_log(&placement.log);
    let log = File::options()
        .create(true)
        .append(true)
        .open(&placement.log)?;
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    // Linux : environnement nettoyé comme celui des programmes du bureau
    // (AppImage, LD_*), comme pour les autres moteurs.
    if cfg!(unix) {
        let (appdir, nvidia_workaround) = desktop_changes();
        scrub(&mut environment, appdir.as_deref(), nvidia_workaround);
    }
    if let Some(fallback) = &placement.fallback {
        crate::image_gen::offer_vulkan_loader(&mut environment, fallback);
    }
    let mut command = Command::new(&placement.bin);
    command
        .args(server_args(spec, port))
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if let Some(dir) = placement.log.parent() {
        command.current_dir(dir);
    }
    isolate(&mut command);
    Ok(command)
}

/// Arguments du serveur : adresse locale, modèle, réglages de décodage de
/// transcribe-rs (voir l'en-tête).
fn server_args(spec: &ModelSpec, port: u16) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string().into(),
        "--model".into(),
        spec.file.clone().into(),
        "--threads".into(),
        SERVER_THREADS.to_string().into(),
        "--beam-size".into(),
        "3".into(),
        "--suppress-nst".into(),
        "--no-speech-thold".into(),
        "0.2".into(),
    ];
    // Langue détectée par défaut (le serveur supposerait l'anglais).
    args.extend(["--language".into(), "auto".into()]);
    args
}

/// Le modèle est sur une carte : whisper.cpp écrit la taille du modèle par
/// périphérique (`whisper_model_load:      Vulkan0 total size =   573.40 MB`).
fn on_card(log: &str) -> bool {
    log.lines().any(|line| {
        line.contains("whisper_model_load")
            && line.contains("total size")
            && !line.contains(" CPU total size")
    })
}

/// Journal du lancement (fin, bornée).
fn read_log(log: &Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(file) = File::open(log) {
        let _ = file.take(256 * 1024).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Dernière ligne d'erreur du serveur (pour le journal de l'app).
fn last_error(log: &Path) -> Option<String> {
    read_log(log)
        .lines()
        .rev()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("error") || lower.contains("failed")
        })
        .map(|line| line.trim().to_owned())
}

/// `GET /health` répond 200 (modèle chargé). L'hôte porte le port : le
/// serveur refuse toute autre valeur (correctif
/// `packaging/whispercpp/0002-refuse-browser-requests.patch`).
fn health(port: u16) -> bool {
    http(
        port,
        format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
        &[],
        Duration::from_secs(2),
    )
    .is_ok_and(|(status, _)| status == 200)
}

/// Requête HTTP/1.1 minimale vers le serveur local (`head` puis `body`),
/// réponse lue jusqu'à la fermeture : (statut, corps).
fn http(port: u16, head: &[u8], body: &[u8], timeout: Duration) -> std::io::Result<(u16, Vec<u8>)> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(head)?;
    stream.write_all(body)?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer)?;
    let split = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("no HTTP header"))?;
    let header = String::from_utf8_lossy(&answer[..split]).into_owned();
    let status = header
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| std::io::Error::other("no HTTP status"))?;
    let mut content = answer[split + 4..].to_vec();
    if header
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        content = unchunk(&content).ok_or_else(|| std::io::Error::other("bad chunked body"))?;
    }
    Ok((status, content))
}

/// Corps « chunked » remis d'un seul tenant.
fn unchunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|window| window == b"\r\n")?;
        let size_text = std::str::from_utf8(&data[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(data.get(..size)?);
        data = data.get(size + 2..)?;
    }
}

/// Son en WAV 16 bits mono à 16 kHz.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Corps multipart de `/inference`.
fn multipart(boundary: &str, samples: &[f32], request: &Request) -> Vec<u8> {
    let mut body = Vec::new();
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    field("response_format", "verbose_json");
    // Sans horodatage par jeton, le serveur ne recoupe pas les passages tous
    // les 60 caractères (au milieu d'un mot) : mêmes passages que le
    // processeur.
    field("token_timestamps", "false");
    field("language", request.language.as_deref().unwrap_or("auto"));
    if let Some(prompt) = request.initial_prompt() {
        field("prompt", &prompt);
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&wav(samples));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Vec<Segment>,
    /// Refus du serveur (paramètre, son illisible).
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct Segment {
    start: f32,
    end: f32,
    text: String,
}

/// Échec d'une requête au serveur.
#[derive(Debug, PartialEq)]
enum Failure {
    /// Le serveur refuse ce morceau (4xx : son illisible…) : le processeur
    /// le fait, le serveur reste.
    Refused(String),
    /// Serveur en panne (arrêté, réponse illisible, 5xx) : arrêté, le
    /// processeur fait la suite.
    Broken(String),
}

/// Transcription d'un morceau par le serveur : texte (passages mis bout à
/// bout, comme transcribe-rs) et passages.
fn infer(port: u16, samples: &[f32], request: &Request) -> Result<(String, Vec<Timed>), Failure> {
    let boundary = "chatonlm-whisper-boundary-7f3a9c";
    let body = multipart(boundary, samples, request);
    let head = format!(
        "POST /inference HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let (status, content) = http(port, head.as_bytes(), &body, REQUEST_TIMEOUT)
        .map_err(|error| Failure::Broken(error.to_string()))?;
    let answer = serde_json::from_slice::<Answer>(&content);
    if (400..500).contains(&status) {
        let reason = answer.ok().and_then(|answer| answer.error);
        return Err(Failure::Refused(
            reason.unwrap_or_else(|| format!("HTTP {status}")),
        ));
    }
    let answer = answer.map_err(|error| Failure::Broken(format!("HTTP {status}: {error}")))?;
    if let Some(error) = answer.error {
        return Err(Failure::Broken(error));
    }
    if status != 200 {
        return Err(Failure::Broken(format!("HTTP {status}")));
    }
    let text: String = if answer.segments.is_empty() {
        answer.text
    } else {
        answer
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    };
    let segments = answer
        .segments
        .into_iter()
        .map(|segment| Timed {
            start: segment.start,
            end: segment.end,
            text: segment.text,
        })
        .collect();
    Ok((text.trim().to_owned(), segments))
}

/// Chargement de Whisper sur le processeur (repli).
pub type CpuLoader = Box<dyn Fn() -> Result<Box<dyn Recognizer>, String> + Send>;

/// Whisper servi par `whisper-server` sur la carte ; au premier échec du
/// serveur, il est arrêté (VRAM rendue) et la suite passe sur le processeur.
pub struct ServerWhisper {
    server: Option<Server>,
    budget: Arc<dyn Budget>,
    cpu: CpuLoader,
    fallback: Option<Box<dyn Recognizer>>,
}

impl ServerWhisper {
    pub fn new(server: Server, budget: Arc<dyn Budget>, cpu: CpuLoader) -> Self {
        Self {
            server: Some(server),
            budget,
            cpu,
            fallback: None,
        }
    }

    /// Arrête le serveur et attend sa fin (fil qui peut bloquer), puis
    /// annonce la VRAM rendue.
    fn release_now(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop_now();
            self.budget.released();
        }
    }

    /// Transcrit par le serveur ; en cas d'échec, par le processeur.
    fn run(&mut self, samples: &[f32], request: &Request) -> Result<(String, Vec<Timed>), String> {
        if let Some(server) = self.server.as_mut() {
            let port = server.port;
            match infer(port, samples, request) {
                Ok(answer) => return Ok(answer),
                Err(Failure::Refused(error)) => {
                    // i18n-ignore : journal.
                    crate::app_log!(
                        "chatonlm: Whisper on the graphics card refused a piece ({error}); this one on the processor"
                    );
                }
                Err(Failure::Broken(error)) => {
                    // i18n-ignore : journal.
                    crate::app_log!(
                        "chatonlm: Whisper on the graphics card failed ({error}); using the processor"
                    );
                    self.release_now();
                }
            }
        }
        if self.fallback.is_none() {
            self.fallback = Some((self.cpu)()?);
        }
        let recognizer = self
            .fallback
            .as_mut()
            .expect("processor Whisper just loaded");
        let piece = recognizer.transcribe_timed(samples, request)?;
        Ok((piece.text, piece.segments))
    }
}

impl Drop for ServerWhisper {
    /// Libéré sans [`Recognizer::release_card`] (inactivité, autre modèle,
    /// fermeture, peut-être depuis une tâche asynchrone) : arrêt lancé en
    /// tâche, la VRAM annoncée rendue une fois le processus fini ; hors de
    /// tout moteur asynchrone (essais), attendu.
    fn drop(&mut self) {
        let Some(server) = self.server.take() else {
            return;
        };
        let budget = self.budget.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            crate::process::spawn_task(async move {
                server.stopping().await;
                budget.released();
            });
        } else {
            server.stop_now();
            budget.released();
        }
    }
}

impl Recognizer for ServerWhisper {
    fn transcribe(&mut self, samples: &[f32], request: &Request) -> Result<String, String> {
        self.run(samples, request)
            .map(|(text, _)| super::engine::clean_transcript(&text))
            .map_err(|error| transcription_failed(&error))
    }

    fn transcribe_timed(&mut self, samples: &[f32], request: &Request) -> Result<Piece, String> {
        let duration = samples.len() as f32 / RATE as f32;
        self.run(samples, request)
            .map(|(text, segments)| Piece::from_parts(&text, segments, duration))
            .map_err(|error| transcription_failed(&error))
    }

    fn on_card(&self) -> bool {
        self.server.is_some()
    }

    fn release_card(&mut self) {
        self.release_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_is_told_from_the_processor_in_the_server_log() {
        assert!(on_card(
            "whisper_model_load:      Vulkan0 total size =   573.40 MB\n"
        ));
        assert!(!on_card(
            "whisper_model_load:      CPU total size =   573.40 MB\n"
        ));
        assert!(!on_card("ggml_vulkan: Found 1 Vulkan devices:\n"));
    }

    #[test]
    fn sound_is_sent_as_16_bit_mono_wav() {
        let bytes = wav(&[0.0, 1.0, -1.0, 2.0]);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), RATE);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
        let values: Vec<i16> = bytes[44..]
            .chunks(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        // Bornes respectées (2.0 ramené à 1.0).
        assert_eq!(values, [0, i16::MAX, -i16::MAX, i16::MAX]);
    }

    #[test]
    fn the_request_names_the_language_or_lets_whisper_detect_it() {
        let body = |language: Option<&str>| {
            let request = Request {
                language: language.map(str::to_owned),
                glossary: vec!["ChatonLM".into()],
                project: Vec::new(),
            };
            String::from_utf8_lossy(&multipart("b", &[0.0], &request)).into_owned()
        };
        let auto = body(None);
        assert!(auto.contains("name=\"language\"\r\n\r\nauto\r\n"), "{auto}");
        assert!(auto.contains("name=\"prompt\"\r\n\r\nGlossary: ChatonLM.\r\n"));
        assert!(auto.contains("name=\"response_format\"\r\n\r\nverbose_json\r\n"));
        assert!(body(Some("fr")).contains("name=\"language\"\r\n\r\nfr\r\n"));
        assert!(auto.ends_with("\r\n--b--\r\n"));
    }

    #[test]
    fn a_chunked_answer_is_put_back_together() {
        assert_eq!(
            unchunk(b"4\r\nWiki\r\n5;x=1\r\npedia\r\n0\r\n\r\n").as_deref(),
            Some(&b"Wikipedia"[..])
        );
        assert_eq!(unchunk(b"4\r\nWi"), None);
    }

    #[test]
    fn vram_asked_covers_the_measured_model() {
        // large-v3 turbo q5_0 : 574 Mo de fichier, 844 Mio mesurés.
        assert!(vram_needed(574_000_000) >= 844 * 1024 * 1024);
        // Sans débordement.
        assert!(vram_needed(u64::MAX) > u64::MAX / 2);
    }

    #[test]
    fn the_binary_is_found_in_the_resources_or_the_local_build() {
        let dir = tempfile::tempdir().unwrap();
        let resources = dir.path().join("resources");
        let dev = dir.path().join("dev");
        let name = format!("whisper-server{}", std::env::consts::EXE_SUFFIX);
        std::fs::create_dir_all(resources.join(DIR)).unwrap();
        std::fs::create_dir_all(&dev).unwrap();
        assert_eq!(locate(None, Some(&resources), Some(&dev)), None);
        let local = dev.join(&name);
        std::fs::write(&local, b"#!/bin/sh\n").unwrap();
        make_executable(&local);
        assert_eq!(
            locate(None, Some(&resources), Some(&dev)),
            Some(local.clone())
        );
        let shipped = resources.join(DIR).join(&name);
        std::fs::write(&shipped, b"#!/bin/sh\n").unwrap();
        make_executable(&shipped);
        assert_eq!(locate(None, Some(&resources), Some(&dev)), Some(shipped));
        // Variable vide ou relative : aucun binaire.
        assert_eq!(
            locate(Some("".as_ref()), Some(&resources), Some(&dev)),
            None
        );
        assert_eq!(
            locate(Some("bin".as_ref()), Some(&resources), Some(&dev)),
            None
        );
        assert_eq!(locate(Some(local.as_os_str()), None, None), Some(local));
    }

    /// Arbitre noté.
    #[derive(Default)]
    struct Noted(Mutex<Vec<&'static str>>);

    impl Budget for Noted {
        fn make_room(&self, _bytes: u64, _long_job: bool) -> bool {
            true
        }
        fn taken(&self, _pid: u32, _bytes: u64) {
            self.0.lock().push("taken");
        }
        fn released(&self) {
            self.0.lock().push("released");
        }
    }

    /// Whisper du processeur, pour le repli.
    #[cfg(unix)]
    struct Cpu;

    #[cfg(unix)]
    impl Recognizer for Cpu {
        fn transcribe(&mut self, _samples: &[f32], _request: &Request) -> Result<String, String> {
            Ok("on the processor".into())
        }
    }

    /// Faux `whisper-server` : `answers` rendues dans l'ordre, une par
    /// connexion (`/inference`), puis plus rien.
    #[cfg(unix)]
    fn fake_server(answers: Vec<(u16, String)>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, answer) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut seen = Vec::new();
                let mut buffer = [0u8; 65536];
                // En-tête, puis le corps annoncé.
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    seen.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&seen).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text
                            .lines()
                            .find_map(|line| line.strip_prefix("Content-Length: "))
                            .map_or(0, |value| value.trim().parse().unwrap());
                        if seen.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if read == 0 {
                        break;
                    }
                }
                requests.push(String::from_utf8_lossy(&seen).into_owned());
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{answer}",
                            answer.len()
                        )
                        .as_bytes(),
                    )
                    .unwrap();
            }
            requests
        });
        (port, thread)
    }

    /// Processus qui tient lieu du serveur (arrêté par le repli).
    #[cfg(unix)]
    fn stand_in() -> (u32, oneshot::Receiver<Option<ExitStatus>>) {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        (child.id(), crate::process::watch_exit(child))
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_card_server_is_stopped_and_the_processor_takes_over() {
        let (port, fake) = fake_server(vec![
            // Texte en lignes (comme le vrai serveur) : refait à partir des
            // passages mis bout à bout, comme sur le processeur.
            (
                200,
                r#"{"text":" Hello there, I am\n dictating a sentence.\n","segments":[{"start":0.0,"end":1.5,"text":" Hello there, I am"},{"start":1.5,"end":3.0,"text":" dictating a sentence."}]}"#.into(),
            ),
            // Morceau refusé : le processeur le fait, le serveur reste.
            (400, r#"{"error":"failed to read audio data"}"#.into()),
            (
                200,
                r#"{"text":" Again.","segments":[{"start":0.0,"end":1.0,"text":" Again."}]}"#.into(),
            ),
            (200, "not json".into()),
        ]);
        let (pid, exited) = stand_in();
        let budget = Arc::new(Noted::default());
        let mut whisper = ServerWhisper::new(
            Server { pid, port, exited },
            budget.clone(),
            Box::new(|| Ok(Box::new(Cpu) as Box<dyn Recognizer>)),
        );
        let request = Request {
            language: Some("en".into()),
            ..Request::default()
        };
        let samples = vec![0.0; RATE as usize * 2];
        // Sur la carte : passages horodatés du serveur.
        let piece = whisper.transcribe_timed(&samples, &request).unwrap();
        assert_eq!(piece.text, "Hello there, I am dictating a sentence.");
        assert_eq!(piece.segments.len(), 2);
        assert_eq!(piece.segments[0].end, 1.5);
        assert!(whisper.on_card());
        assert_eq!(
            whisper.transcribe(&samples, &request).unwrap(),
            "on the processor"
        );
        assert!(whisper.on_card());
        assert!(budget.0.lock().is_empty());
        assert_eq!(whisper.transcribe(&samples, &request).unwrap(), "Again.");
        // Réponse illisible : serveur arrêté, VRAM rendue, le processeur
        // refait le morceau, puis fait la suite.
        assert_eq!(
            whisper.transcribe(&samples, &request).unwrap(),
            "on the processor"
        );
        assert!(!whisper.on_card());
        assert_eq!(*budget.0.lock(), ["released"]);
        assert_eq!(
            whisper.transcribe(&samples, &request).unwrap(),
            "on the processor"
        );
        let requests = fake.join().unwrap();
        assert!(requests[0].starts_with("POST /inference HTTP/1.1"));
        assert!(requests[0].contains("name=\"language\"\r\n\r\nen\r\n"));
        assert!(requests[0].contains("name=\"token_timestamps\"\r\n\r\nfalse\r\n"));
        // Le processus du serveur est bien arrêté.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Path::new(&format!("/proc/{pid}")).exists()
            && std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|stat| !stat.contains(") Z "))
        {
            assert!(Instant::now() < deadline, "server process still running");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(whisper);
        assert_eq!(*budget.0.lock(), ["released"]);
    }

    /// Vrai `whisper-server` (construction locale) et vrai modèle, sur la
    /// carte : à lancer à la main (`… whisper_server -- --ignored`).
    #[test]
    #[ignore = "vrai whisper-server, modèle et son locaux, carte graphique"]
    fn the_real_server_transcribes_on_the_card() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(".tools");
        let bin = locate(None, None, Some(&root.join("runtimes").join(DIR)))
            .expect("scripts/build-whisper-server.sh");
        let model = root.join("speech-models/whisper/ggml-base.bin");
        let sound = std::fs::read(root.join("whispercpp-proof/src/samples/jfk.wav")).unwrap();
        let data = sound
            .windows(4)
            .position(|window| window == b"data")
            .unwrap()
            + 8;
        let samples: Vec<f32> = sound[data..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
            .collect();
        let logs = tempfile::tempdir().unwrap();
        let budget = Arc::new(Noted::default());
        let placement = Placement::new(
            bin,
            Some(root.join("runtimes/llama-server/fallback")),
            logs.path().join("whisper-server.log"),
            budget.clone(),
        );
        let spec = ModelSpec {
            id: "whisper-base".into(),
            engine: super::super::models::SpeechEngine::Whisper,
            dir: model.parent().unwrap().to_path_buf(),
            file: model,
        };
        let server = start(&placement, &spec, false)
            .unwrap()
            .expect("loaded on the graphics card");
        assert_eq!(*budget.0.lock(), ["taken"]);
        let mut whisper = ServerWhisper::new(
            server,
            budget.clone(),
            Box::new(|| Err("no processor".into())),
        );
        let request = Request {
            language: Some("en".into()),
            ..Request::default()
        };
        let piece = whisper.transcribe_timed(&samples, &request).unwrap();
        assert!(
            piece.text.contains("ask not what your country"),
            "{}",
            piece.text
        );
        assert!(!piece.segments.is_empty());
        drop(whisper);
        assert_eq!(*budget.0.lock(), ["taken", "released"]);
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = path;
    }
}
