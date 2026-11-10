//! Lecture du journal d'un `llama-server` (fonctions pures, essayées sur de
//! vrais journaux de b12110 : `engine/testdata/`) : mémoire prise par
//! périphérique au chargement, couches sur la carte, erreurs qui expliquent
//! un échec, et ce qui justifie un nouvel essai sur le processeur.
//!
//! Écriture bornée (relecture de L3, M6) : `-lv 4` écrit environ 3 Ko par
//! requête pendant toute la vie du processus (l'autocomplétion en ferait des
//! dizaines de Mo). Un fichier par modèle et par lancement
//! (`llama-<modèle>.log`, le précédent gardé une fois en `.1`), que l'app
//! remplit elle-même depuis les sorties du processus ([`relay`]) et fait
//! repartir à zéro au-delà de [`MAX_LOG`] ([`Capped`], l'ancien en `.1`) :
//! la mémoire n'est lue qu'au chargement ; la fin (erreur d'une mort) reste
//! dans le fichier courant.
//!
//! Format d'une ligne : `<minutes>.<secondes>.<ms>.<µs> <niveau> <texte>`
//! (`I`, `W`, `E`, `D`) ; quelques messages de ggml et de Vulkan arrivent
//! sans cet en-tête (sortie d'erreur directe).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use parking_lot::Mutex;

const MIB: f64 = 1024.0 * 1024.0;

/// Taille d'un journal, au plus : au-delà, il passe en `.1` (le journal du
/// lancement précédent est alors perdu) et repart à zéro, pour que la fin
/// (l'erreur d'un processus de longue vie) reste lisible (relecture L5a, M2).
pub const MAX_LOG: u64 = 16 * 1024 * 1024;

/// Ligne écrite à la coupure, quand le journal ne peut pas repartir
/// (journal technique, en anglais).
pub const CUT_LINE: &str =
    "\n--- ChatonLM: log cut here (16 MiB), the rest of this run is not kept ---\n";

/// Première ligne d'un journal reparti à zéro (journal technique, en
/// anglais).
pub const CONTINUED_LINE: &str =
    "--- ChatonLM: log continued (the previous 16 MiB of this run are in the .1 file) ---\n";

/// Nom du journal d'un modèle (`llama-<modèle>.log`) : l'identifiant, tout
/// caractère hors lettres ASCII, chiffres, `.`, `-` et `_` remplacé par `_`
/// (`/` et `:` des identifiants, `@` des modèles repris d'Ollama : un nom de
/// fichier valable sous Linux et Windows).
pub fn file_name(model: &str) -> String {
    let name: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("llama-{name}.log")
}

/// Nouveau fichier d'un journal plein (l'ancien mis de côté) ; `Err` : il
/// ne peut pas repartir.
pub type Reopen<W> = Box<dyn FnMut() -> std::io::Result<W> + Send>;

/// Journal borné : ce qui tient sous la limite est écrit ; à la limite, le
/// fichier repart à zéro par `reopen` (l'ancien en `.1`, [`CONTINUED_LINE`]
/// en tête) ; sans `reopen`, ou s'il échoue, [`CUT_LINE`] une fois, puis
/// plus rien.
pub struct Capped<W> {
    out: W,
    written: u64,
    limit: u64,
    cut: bool,
    reopen: Option<Reopen<W>>,
}

impl<W: Write> Capped<W> {
    pub fn new(out: W, limit: u64) -> Self {
        Self {
            out,
            written: 0,
            limit,
            cut: false,
            reopen: None,
        }
    }

    /// Journal qui repart à zéro par `reopen` à chaque limite atteinte.
    pub fn rotating(out: W, limit: u64, reopen: Reopen<W>) -> Self {
        Self {
            reopen: Some(reopen),
            ..Self::new(out, limit)
        }
    }

    /// Écrit `bytes`, coupés à la limite (erreurs d'écriture ignorées : le
    /// journal ne doit jamais arrêter le processus).
    pub fn write(&mut self, mut bytes: &[u8]) {
        while !self.cut && !bytes.is_empty() {
            let room =
                usize::try_from(self.limit.saturating_sub(self.written)).unwrap_or(usize::MAX);
            let kept = &bytes[..bytes.len().min(room)];
            let _ = self.out.write_all(kept);
            self.written += kept.len() as u64;
            bytes = &bytes[kept.len()..];
            if bytes.is_empty() {
                return;
            }
            let _ = self.out.flush();
            match self.reopen.as_mut().map(|reopen| reopen()) {
                Some(Ok(fresh)) => {
                    self.out = fresh;
                    let _ = self.out.write_all(CONTINUED_LINE.as_bytes());
                    self.written = CONTINUED_LINE.len() as u64;
                }
                Some(Err(_)) | None => {
                    let _ = self.out.write_all(CUT_LINE.as_bytes());
                    self.cut = true;
                }
            }
        }
    }

    /// Ce qui a été écrit (essais).
    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.out
    }
}

/// Reprise d'un journal plein sur disque : `log` passe en `.1` (remplace
/// celui du lancement précédent), un nouveau `log` est créé. Le fichier
/// encore ouvert se renomme aussi sous Windows (la bibliothèque standard
/// l'ouvre avec `FILE_SHARE_DELETE`).
pub fn reopen_file(log: &Path) -> Reopen<File> {
    let log = log.to_owned();
    Box::new(move || {
        let mut previous = log.file_name().unwrap_or_default().to_os_string();
        previous.push(".1");
        std::fs::rename(&log, log.with_file_name(previous))?;
        File::options()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log)
    })
}

/// Recopie `source` (sortie ou erreur du processus) dans le journal partagé
/// jusqu'à la fin du flux ; au-delà de la limite, la lecture continue (et
/// jette) : le processus n'est jamais bloqué sur un tube plein.
pub fn relay<W: Write>(mut source: impl Read, sink: &Mutex<Capped<W>>) {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match source.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => sink.lock().write(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Journal relu au plus (le chargement tient en quelques centaines de
/// lignes ; ensuite, une vingtaine par requête).
pub const MAX_READ: u64 = 4 * 1024 * 1024;

/// Erreurs gardées pour expliquer un échec.
const MAX_ERRORS: usize = 3;

/// Mémoire d'un processus d'après son journal de chargement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// Tout ce que le chargement a réservé (RAM et VRAM), en octets.
    pub total: u64,
    /// Part sur une carte graphique (ni `CPU…` ni `…_Host`).
    pub gpu: u64,
    /// `offloaded N/M layers to GPU` ; `None` sans la ligne (processeur
    /// seul, aucune carte Vulkan).
    pub gpu_layers: Option<u32>,
    pub total_layers: Option<u32>,
    /// Au moins une ligne de mémoire lue.
    pub measured: bool,
}

/// Périphérique d'une réserve : sur une carte (`Vulkan0`, `CUDA0`) ou en
/// mémoire vive (`CPU`, `CPU_Mapped`, `CPU_REPACK`, `Vulkan_Host`,
/// `CUDA_Host`).
fn on_gpu(device: &str) -> bool {
    !(device.starts_with("CPU") || device.ends_with("_Host") || device == "Host")
}

/// Texte d'une ligne sans son horodatage ni son niveau ; niveau `None` pour
/// une ligne sans en-tête.
fn split(line: &str) -> (Option<char>, &str) {
    let mut parts = line.splitn(3, ' ');
    let (Some(stamp), Some(level)) = (parts.next(), parts.next()) else {
        return (None, line.trim());
    };
    let stamped = !stamp.is_empty() && stamp.bytes().all(|c| c.is_ascii_digit() || c == b'.');
    let mut chars = level.chars();
    match (stamped, chars.next(), chars.next()) {
        (true, Some(level), None) => (Some(level), parts.next().unwrap_or("").trim()),
        _ => (None, line.trim()),
    }
}

/// Octets d'une taille en Mio (« 372.65 »).
fn mib(text: &str) -> Option<u64> {
    let value: f64 = text.trim().parse().ok()?;
    (value.is_finite() && value >= 0.0).then(|| (value * MIB).round() as u64)
}

/// Mémoire réservée au chargement : lignes `<dev> model|KV|RS|compute|output
/// buffer size = N MiB` (poids, cache, état récurrent, calcul), taille du
/// projecteur (`load_hparams: model size: N MiB`, sur le périphérique de
/// `CLIP using <dev> backend`), couches sur la carte. Les tableaux de
/// prévision (`common_memory_breakdown_print`) sont ignorés : ce sont des
/// projections, et la fin du processus les réécrit.
pub fn memory(log: &str) -> Memory {
    let mut memory = Memory::default();
    let mut clip_device: Option<String> = None;
    for line in log.lines() {
        let (_, text) = split(line);
        if let Some(rest) = text.split_once("offloaded ").map(|(_, rest)| rest)
            && let Some((counts, _)) = rest.split_once(" layers to GPU")
            && let Some((done, all)) = counts.split_once('/')
        {
            memory.gpu_layers = done.trim().parse().ok();
            memory.total_layers = all.trim().parse().ok();
            continue;
        }
        if let Some((_, rest)) = text.split_once("CLIP using ")
            && let Some(device) = rest.strip_suffix(" backend")
        {
            clip_device = Some(device.trim().to_owned());
            continue;
        }
        if text.starts_with("load_hparams:")
            && let Some((_, size)) = text.split_once("model size:")
            && let Some(bytes) = size.trim().strip_suffix("MiB").and_then(mib)
        {
            memory.total += bytes;
            if clip_device.as_deref().is_some_and(on_gpu) {
                memory.gpu += bytes;
            }
            memory.measured = true;
            continue;
        }
        let Some((head, size)) = text.split_once(" buffer size =") else {
            continue;
        };
        let Some(bytes) = size.trim().strip_suffix("MiB").and_then(mib) else {
            continue;
        };
        // « llama_kv_cache:    Vulkan0 KV » : le périphérique est l'avant-dernier
        // mot, le genre de réserve le dernier.
        let mut words = head.split_whitespace().rev();
        let (Some(kind), Some(device)) = (words.next(), words.next()) else {
            continue;
        };
        if !matches!(kind, "model" | "KV" | "RS" | "compute" | "output") {
            continue;
        }
        memory.total += bytes;
        if on_gpu(device) {
            memory.gpu += bytes;
        }
        memory.measured = true;
    }
    memory
}

/// Dernières erreurs du journal ([`MAX_ERRORS`] au plus, de la plus ancienne
/// à la plus récente) : lignes de niveau `E`, et lignes sans en-tête qui
/// disent une erreur (ggml, Vulkan, exception C++).
pub fn errors(log: &str) -> Option<String> {
    let found: Vec<&str> = log
        .lines()
        .filter_map(|line| match split(line) {
            (Some('E'), text) => Some(text),
            (None, text) if says_error(text) => Some(text),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect();
    let kept = &found[found.len().saturating_sub(MAX_ERRORS)..];
    (!kept.is_empty()).then(|| kept.join("; "))
}

fn says_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["error", "failed", "terminate called", "exception", "abort"]
        .iter()
        .any(|word| lower.contains(word))
}

/// Le journal accuse Vulkan : une erreur (niveau `E`, ou sans en-tête qui en
/// dit une) qui nomme Vulkan, ou un code d'erreur de Vulkan
/// (`ErrorDeviceLost`, `VK_ERROR_…`, `vk::…Error`).
pub fn vulkan_failed(log: &str) -> bool {
    log.lines().any(|line| {
        let (level, text) = split(line);
        let lower = text.to_ascii_lowercase();
        let vulkan_code = text.contains("VK_ERROR")
            || (text.contains("vk::") && lower.contains("error"))
            || [
                "ErrorDeviceLost",
                "ErrorOutOfDeviceMemory",
                "ErrorInitializationFailed",
            ]
            .iter()
            .any(|code| text.contains(code));
        let names_vulkan = lower.contains("vulkan") || lower.contains("ggml_vk");
        vulkan_code
            || (names_vulkan
                && match level {
                    Some('E') => true,
                    None => says_error(text),
                    _ => false,
                })
    })
}

/// Fin d'un processus, réduite à ce qui décide d'un nouvel essai.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitFacts {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// Le processus a planté (et pas seulement refusé le modèle, code 1) :
/// signal de faute (SIGILL, SIGABRT, SIGBUS, SIGFPE, SIGSEGV) sous Unix ;
/// sous Windows, une exception du noyau (code `0xC000_0000` et au-delà :
/// violation d'accès, dépassement de pile, `0xC0000409`) ou `abort()`
/// (code 3).
pub fn crashed(exit: ExitFacts, windows: bool) -> bool {
    if let Some(signal) = exit.signal {
        return matches!(signal, 4 | 6 | 7 | 8 | 11);
    }
    match exit.code {
        Some(code) if windows => code as u32 >= 0xC000_0000 || code == 3,
        _ => false,
    }
}

/// Nouvel essai sur le processeur après un lancement raté sur la carte :
/// erreur de Vulkan dans le journal, ou plantage au chargement.
pub fn retry_on_cpu(log: &str, exit: ExitFacts, windows: bool) -> bool {
    vulkan_failed(log) || crashed(exit, windows)
}

/// Fin du journal de ce lancement (au plus [`MAX_READ`] octets, lus depuis
/// le début : le chargement y est), en texte.
pub fn read(log: &Path) -> String {
    let Ok(mut file) = File::open(log) else {
        return String::new();
    };
    let length = file.metadata().map_or(0, |meta| meta.len());
    let mut bytes = Vec::new();
    let _ = (&mut file).take(MAX_READ).read_to_end(&mut bytes);
    // Journal plus long : sa fin aussi (une erreur arrive en dernier).
    if length > MAX_READ && file.seek(SeekFrom::Start(length - MAX_READ / 4)).is_ok() {
        bytes.extend_from_slice(b"\n");
        let _ = file.take(MAX_READ / 4).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Le journal recopié contient la fin du chargement (`server_run_i: model
/// loaded` de b12110, écrite avant que le serveur écoute) : la mémoire du
/// chargement y est lisible.
pub fn load_finished(log: &str) -> bool {
    log.contains("model loaded")
}

/// Le port n'a pas pu être lié (pris par un autre entre son choix et la
/// liaison) : `couldn't bind HTTP server socket` de b12110.
pub fn port_taken(log: &str) -> bool {
    log.contains("couldn't bind HTTP server socket")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPU: &str = include_str!("testdata/vulkan.log");
    const CPU: &str = include_str!("testdata/cpu.log");
    const NO_VULKAN: &str = include_str!("testdata/cpu-novk.log");
    const VISION: &str = include_str!("testdata/vision.log");

    fn mib(n: f64) -> u64 {
        (n * MIB).round() as u64
    }

    /// Ligne de b12110 (`server-http.cpp`) quand le port est pris.
    #[test]
    fn a_taken_port_is_recognised() {
        let log = "0.00.010.000 E srv  init: couldn't bind HTTP server socket, hostname: \
                   127.0.0.1, port: 40123\n0.00.010.001 E srv  server_run_i: exiting due to \
                   HTTP server error\n";
        assert!(port_taken(log));
        assert!(!port_taken(GPU));
        assert!(!port_taken(CPU));
    }

    /// Qwen3-0.6B Q4_K_M, `-c 4096`, RTX 3090 (Vulkan), b12110 : 846 Mio sur la
    /// carte selon llama.cpp (poids 372, cache 448, calcul 26), 858 Mio vus par
    /// `nvidia-smi`.
    #[test]
    fn the_memory_of_a_model_on_the_card_is_read() {
        let memory = memory(GPU);
        assert!(memory.measured);
        assert_eq!(memory.gpu_layers, Some(29));
        assert_eq!(memory.total_layers, Some(29));
        assert_eq!(memory.gpu, mib(372.65) + mib(448.0) + mib(26.01));
        assert_eq!(
            memory.total,
            memory.gpu + mib(121.71) + mib(0.58) + mib(12.02)
        );
        let gpu_mib = memory.gpu / 1024 / 1024;
        assert!((840..=850).contains(&gpu_mib), "{gpu_mib}");
    }

    /// `-ngl 0` : poids et cache en mémoire vive, seul le calcul sur la carte.
    #[test]
    fn the_memory_of_a_model_on_the_processor_is_read() {
        let memory = memory(CPU);
        assert_eq!(memory.gpu_layers, Some(0));
        assert_eq!(memory.gpu, mib(29.69));
        assert!(memory.total > mib(372.65) + mib(224.0));
        // Aucune carte Vulkan (`GGML_VK_VISIBLE_DEVICES=none`) : ni ligne de
        // couches, ni mémoire sur la carte.
        let memory = super::memory(NO_VULKAN);
        assert!(memory.measured);
        assert_eq!(memory.gpu, 0);
        assert_eq!(memory.gpu_layers, None);
        assert!(memory.total > mib(121.0) + mib(224.0));
    }

    /// SmolVLM-256M avec son projecteur : le projecteur compte, sur la carte
    /// qui l'a pris.
    #[test]
    fn the_projector_counts_on_its_device() {
        let memory = memory(VISION);
        assert_eq!(memory.gpu_layers, Some(31));
        assert_eq!(
            memory.gpu,
            mib(136.47) + mib(90.0) + mib(17.51) + mib(98.95) + mib(21.0)
        );
        // Le même journal, projecteur sur le processeur.
        let on_cpu = VISION.replace("CLIP using Vulkan0 backend", "CLIP using CPU backend");
        assert_eq!(memory.gpu - self::memory(&on_cpu).gpu, mib(98.95));
    }

    #[test]
    fn nothing_is_measured_in_an_empty_or_foreign_log() {
        assert_eq!(memory(""), Memory::default());
        assert!(!memory("hello\nmodel buffer size = lots\n").measured);
        // Tableaux de prévision : ignorés.
        let projections = GPU
            .lines()
            .filter(|line| line.contains("breakdown") || line.contains("projected"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!projections.is_empty());
        assert!(!memory(&projections).measured);
    }

    #[test]
    fn errors_explain_a_failed_load() {
        let log = "\
0.00.001.000 I srv  llama_server: initializing ...
0.00.002.000 E llama_model_load: error loading model: unknown model architecture: 'mistral3'
0.00.002.100 E srv    load_model: failed to load model, '/m/a.gguf'
0.00.002.200 E srv  server_run_i: exiting due to model loading error
0.00.002.300 I srv    operator(): cleaning up before exit...
";
        // Les trois dernières, de la plus ancienne à la plus récente : la cause
        // vient avant ses conséquences.
        assert_eq!(
            errors(log).as_deref(),
            Some(
                "llama_model_load: error loading model: unknown model architecture: 'mistral3'; \
                 srv    load_model: failed to load model, '/m/a.gguf'; srv  server_run_i: \
                 exiting due to model loading error"
            )
        );
        let four = format!("0.00.001.000 E first error\n{log}");
        assert!(!errors(&four).unwrap().contains("first error"));
        assert_eq!(errors(GPU), None);
        // Sans en-tête : gardée si elle dit une erreur.
        assert_eq!(
            errors("ggml_vulkan: Invalid device index 3 in GGML_VK_VISIBLE_DEVICES.\nhello\n"),
            None
        );
        assert_eq!(
            errors("terminate called after throwing an instance of 'vk::DeviceLostError'\n")
                .as_deref(),
            Some("terminate called after throwing an instance of 'vk::DeviceLostError'")
        );
        // Un modèle refusé ne met pas Vulkan en cause.
        assert!(!vulkan_failed(log));
    }

    #[test]
    fn vulkan_failures_are_recognised() {
        for log in [
            "terminate called after throwing an instance of 'vk::DeviceLostError'\n  what():  vk::Queue::submit: ErrorDeviceLost\n",
            "0.00.100.000 E ggml_vulkan: Device memory allocation of size 123 failed.\n",
            "ggml_vulkan: vk::Device::createComputePipeline: ErrorInitializationFailed\n",
            "0.00.100.000 E llama_init: failed to allocate buffer: VK_ERROR_OUT_OF_DEVICE_MEMORY\n",
            "ggml_vulkan: Error: Vulkan 1.2 required.\n",
        ] {
            assert!(vulkan_failed(log), "{log}");
        }
        for log in [
            GPU,
            CPU,
            VISION,
            "0.00.100.000 I ggml_vulkan: Found 1 Vulkan devices:\n",
            "0.00.100.000 W resolve_fused_ops: layer 0 is assigned to device Vulkan0 but Flash Attention is not supported\n",
        ] {
            assert!(!vulkan_failed(log));
        }
    }

    #[test]
    fn crashes_are_told_from_refusals() {
        let signal = |signal| ExitFacts {
            code: None,
            signal: Some(signal),
        };
        let code = |code| ExitFacts {
            code: Some(code),
            signal: None,
        };
        for faulty in [4, 6, 7, 8, 11] {
            assert!(crashed(signal(faulty), false), "{faulty}");
        }
        // SIGTERM, SIGKILL : arrêtés, pas plantés.
        assert!(!crashed(signal(15), false));
        assert!(!crashed(signal(9), false));
        // Modèle refusé : code 1.
        assert!(!crashed(code(1), false));
        assert!(!crashed(code(1), true));
        // Windows : exceptions du noyau, abort().
        assert!(crashed(code(0xC000_0005_u32 as i32), true));
        assert!(crashed(code(0xC000_0409_u32 as i32), true));
        assert!(crashed(code(3), true));
        assert!(!crashed(code(3), false));
        assert!(!crashed(ExitFacts::default(), true));
        assert!(retry_on_cpu("", signal(11), false));
        assert!(retry_on_cpu(
            "0.00.1.0 E ggml_vulkan: device lost\n",
            code(1),
            false
        ));
        assert!(!retry_on_cpu(
            "0.00.1.0 E failed to load model\n",
            code(1),
            false
        ));
    }

    #[test]
    fn the_end_of_the_load_is_seen() {
        for log in [GPU, CPU, NO_VULKAN, VISION] {
            assert!(load_finished(log));
            let start = &log[..log.find("model loaded").unwrap()];
            assert!(!load_finished(start));
        }
    }

    #[test]
    fn each_model_has_its_own_log_file() {
        assert_eq!(
            file_name("Qwen/Qwen3-0.6B-GGUF:Q4_K_M"),
            "llama-Qwen_Qwen3-0.6B-GGUF_Q4_K_M.log"
        );
        assert_eq!(
            file_name("@ollama/qwen3:0.6b"),
            "llama-_ollama_qwen3_0.6b.log"
        );
        assert_eq!(file_name("a\\b c"), "llama-a_b_c.log");
    }

    /// M6 : au-delà de la limite, le reste est jeté (une ligne le dit), et
    /// le début (la mémoire du chargement) reste.
    #[test]
    fn the_log_is_cut_beyond_its_limit() {
        let mut capped = Capped::new(Vec::new(), 10);
        capped.write(b"load: ");
        capped.write(b"vram 42\nmore");
        capped.write(b"never");
        let written = String::from_utf8(capped.into_inner()).unwrap();
        assert_eq!(written, format!("load: vram{CUT_LINE}"));
        // Sous la limite : tout.
        let mut capped = Capped::new(Vec::new(), MAX_LOG);
        capped.write(GPU.as_bytes());
        assert_eq!(capped.into_inner(), GPU.as_bytes());
    }

    /// M2 (relecture L5a) : à la limite, le journal passe en `.1` et repart à
    /// zéro ; la dernière erreur d'un processus de longue vie reste lisible
    /// (`read`, `vulkan_failed`).
    #[test]
    fn a_full_log_starts_again_and_keeps_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("llama-m.log");
        let old = dir.path().join("llama-m.log.1");
        std::fs::write(&old, "previous run").unwrap();
        let file = File::create(&log).unwrap();
        let limit = 4096;
        let mut capped = Capped::rotating(file, limit, reopen_file(&log));
        capped.write(GPU.as_bytes());
        let request = "slot update_slots: id  0 | task 7 | prompt done\n";
        for _ in 0..(2 * limit as usize / request.len()) {
            capped.write(request.as_bytes());
        }
        capped.write(b"ggml_vulkan: vk::Device::waitForFences: ErrorDeviceLost\n");
        drop(capped);
        let text = read(&log);
        assert!(text.starts_with(CONTINUED_LINE), "{text}");
        assert!(text.contains("ErrorDeviceLost"));
        assert!(vulkan_failed(&text));
        assert!(!text.contains(CUT_LINE.trim()));
        // Le début de ce lancement est passé en `.1` (le précédent remplacé).
        let previous = std::fs::read_to_string(&old).unwrap();
        assert!(!previous.contains("previous run"));
        assert!(std::fs::metadata(&log).unwrap().len() <= limit);
        // Sans reprise possible (renommage refusé) : coupé comme avant.
        let refused: Reopen<Vec<u8>> = Box::new(|| Err(std::io::Error::other("rename refused")));
        let mut capped = Capped::rotating(Vec::new(), 10, refused);
        capped.write(b"0123456789abc");
        capped.write(b"never");
        let written = String::from_utf8(capped.into_inner()).unwrap();
        assert_eq!(written, format!("0123456789{CUT_LINE}"));
    }

    /// La recopie lit tout le flux, même au-delà de la limite (sinon le
    /// processus bloquerait sur un tube plein), et n'écrit que le début.
    #[test]
    fn the_relay_drains_the_whole_stream() {
        struct Counted<'a>(&'a [u8], usize);
        impl Read for Counted<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = out.len().min(self.0.len()).min(7);
                out[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                self.1 += n;
                Ok(n)
            }
        }
        let data = "line\n".repeat(1_000);
        let sink = Mutex::new(Capped::new(Vec::new(), 100));
        let mut source = Counted(data.as_bytes(), 0);
        relay(&mut source, &sink);
        assert_eq!(source.1, data.len(), "everything was read");
        let written = sink.into_inner().into_inner();
        assert_eq!(written.len(), 100 + CUT_LINE.len());
        assert!(written.starts_with(b"line\nline\n"));
    }

    #[test]
    fn the_log_is_read_within_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("llama-0.log");
        assert_eq!(read(&log), "");
        std::fs::write(&log, GPU).unwrap();
        assert_eq!(read(&log), GPU);
        // Très long : le début, puis la fin.
        let mut big = String::from("first line\n");
        big.push_str(&"x".repeat(MAX_READ as usize * 2));
        big.push_str("\nlast line\n");
        std::fs::write(&log, &big).unwrap();
        let text = read(&log);
        assert!(text.starts_with("first line"));
        assert!(text.ends_with("last line\n"));
        assert!(text.len() as u64 <= MAX_READ + MAX_READ / 4 + 1);
    }
}
