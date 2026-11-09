//! Lecture du journal d'un `llama-server` (fonctions pures, essayées sur de
//! vrais journaux de b12110 : `engine/testdata/`) : mémoire prise par
//! périphérique au chargement, couches sur la carte, erreurs qui expliquent
//! un échec, et ce qui justifie un nouvel essai sur le processeur.
//!
//! Format d'une ligne : `<minutes>.<secondes>.<ms>.<µs> <niveau> <texte>`
//! (`I`, `W`, `E`, `D`) ; quelques messages de ggml et de Vulkan arrivent
//! sans cet en-tête (sortie d'erreur directe).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MIB: f64 = 1024.0 * 1024.0;

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
