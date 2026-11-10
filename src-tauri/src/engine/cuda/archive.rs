//! Extraction du pack CUDA (.tar.gz sous Linux, .zip sous Windows) et des
//! archives de NVIDIA (.tar.xz, .zip) : seuls les fichiers ordinaires retenus
//! par une règle sont écrits, à plat, sous un nom choisi par l'app (jamais
//! un chemin venu de l'archive). Liens, dossiers et tout le reste (en-têtes,
//! bibliothèques statiques, `stubs/`) sont ignorés. Logique pure : les deux
//! formats sont essayés sous Linux.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Platform;

/// Format d'une archive, d'après son nom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    TarGz,
    TarXz,
    Zip,
}

impl Kind {
    pub fn of(name: &str) -> Option<Kind> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            Some(Kind::TarGz)
        } else if lower.ends_with(".tar.xz") {
            Some(Kind::TarXz)
        } else if lower.ends_with(".zip") {
            Some(Kind::Zip)
        } else {
            None
        }
    }
}

/// Fichiers du pack de ChatonLM (à plat), moteur compris.
pub fn pack_files(platform: Platform) -> [&'static str; 5] {
    [
        platform.engine_file(),
        "VERSION",
        "SHA256SUMS",
        "LICENSE-llama.cpp.txt",
        "LICENSE-CCCL.txt",
    ]
}

/// Règle du pack : un fichier à la racine de l'archive, parmi
/// [`pack_files`].
pub fn pack_pick(platform: Platform) -> impl Fn(&str) -> Option<String> {
    move |path: &str| {
        let path = path.strip_prefix("./").unwrap_or(path);
        pack_files(platform)
            .into_iter()
            .find(|name| *name == path)
            .map(str::to_owned)
    }
}

/// Licence de NVIDIA rangée à côté des bibliothèques.
pub const NVIDIA_LICENSE: &str = "LICENSE-NVIDIA-CUDA.txt";

/// Bibliothèques de NVIDIA gardées (noms d'arrivée) : celles de
/// `libggml-cuda.so` / `ggml-cuda.dll` et de cuBLAS.
pub fn nvidia_files(platform: Platform, major: u32) -> Vec<String> {
    match platform {
        Platform::Linux => ["libcudart", "libcublas", "libcublasLt"]
            .iter()
            .map(|stem| format!("{stem}.so.{major}"))
            .collect(),
        Platform::Windows => ["cudart64", "cublas64", "cublasLt64"]
            .iter()
            .map(|stem| format!("{stem}_{major}.dll"))
            .collect(),
    }
}

/// Règle d'une archive de NVIDIA : `<racine>/lib/<stem>.so.<major>[.x.y.z]`
/// (Linux ; le fichier versionné, que désigne le lien du SONAME) ou
/// `<racine>/bin[/x64]/<stem>_<major>.dll` (Windows), écrit sous le nom du
/// SONAME / de la DLL ; avec `license`, aussi `<racine>/LICENSE`.
pub fn nvidia_pick(
    platform: Platform,
    major: u32,
    license: bool,
) -> impl Fn(&str) -> Option<String> {
    let wanted = nvidia_files(platform, major);
    move |path: &str| {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        match parts.as_slice() {
            [_, "LICENSE"] if license => Some(NVIDIA_LICENSE.to_owned()),
            [_, "lib", name] if platform == Platform::Linux => wanted
                .iter()
                .find(|soname| {
                    name.strip_prefix(soname.as_str()).is_some_and(|rest| {
                        rest.is_empty()
                            || (rest.starts_with('.')
                                && rest[1..].split('.').all(|part| {
                                    !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())
                                }))
                    })
                })
                .cloned(),
            [_, "bin", name] | [_, "bin", "x64", name] if platform == Platform::Windows => wanted
                .iter()
                .find(|dll| dll.eq_ignore_ascii_case(name))
                .cloned(),
            _ => None,
        }
    }
}

/// Copie par morceaux, arrêtée dès que `cancel` est levé.
fn copy(mut from: impl Read, to: &mut impl Write, cancel: &AtomicBool) -> io::Result<u64> {
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        let read = match from.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        to.write_all(&buffer[..read])?;
        total += read as u64;
    }
}

/// Erreur d'une extraction annulée.
pub fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

/// Nom d'arrivée sûr : un nom de fichier seul.
fn safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

/// Écrit `from` sous `dest/name` (fichier neuf), une fois.
fn write_entry(
    from: impl Read,
    dest: &Path,
    name: &str,
    written: &mut HashSet<String>,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if !safe_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsafe name: {name}"),
        ));
    }
    if !written.insert(name.to_owned()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("duplicate entry: {name}"),
        ));
    }
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(dest.join(name))?;
    copy(from, &mut file, cancel)?;
    file.sync_all()
}

/// Extrait de `archive` les fichiers ordinaires que `pick` retient (chemin
/// dans l'archive, séparateur `/` → nom d'arrivée) dans `dest` (qui existe) ;
/// rend les noms écrits.
pub fn extract(
    archive: &Path,
    kind: Kind,
    dest: &Path,
    pick: &dyn Fn(&str) -> Option<String>,
    cancel: &AtomicBool,
) -> io::Result<HashSet<String>> {
    let mut written = HashSet::new();
    let file = BufReader::new(File::open(archive)?);
    match kind {
        Kind::TarGz => extract_tar(
            flate2::read::GzDecoder::new(file),
            dest,
            pick,
            &mut written,
            cancel,
        )?,
        Kind::TarXz => extract_tar(
            lzma_rust2::XzReader::new(file, true),
            dest,
            pick,
            &mut written,
            cancel,
        )?,
        Kind::Zip => {
            let mut zip = zip::ZipArchive::new(file).map_err(io::Error::other)?;
            for index in 0..zip.len() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                let entry = zip.by_index(index).map_err(io::Error::other)?;
                if !entry.is_file() {
                    continue;
                }
                let path = entry.name().map_err(io::Error::other)?.replace('\\', "/");
                if let Some(name) = pick(&path) {
                    write_entry(entry, dest, &name, &mut written, cancel)?;
                }
            }
        }
    }
    Ok(written)
}

fn extract_tar(
    reader: impl Read,
    dest: &Path,
    pick: &dyn Fn(&str) -> Option<String>,
    written: &mut HashSet<String>,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries()? {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        let entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        if let Some(name) = pick(&path) {
            write_entry(entry, dest, &name, written, cancel)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Archive .tar (gz ou xz) de `files` (chemin, contenu ; contenu `None` :
    /// lien symbolique vers le fichier précédent).
    pub fn tar_bytes(files: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut previous = String::new();
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            match content {
                Some(bytes) => {
                    header.set_size(bytes.len() as u64);
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_mode(0o755);
                    header.set_cksum();
                    builder.append_data(&mut header, path, *bytes).unwrap();
                }
                None => {
                    header.set_size(0);
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_cksum();
                    builder
                        .append_link(&mut header, path, previous.rsplit('/').next().unwrap())
                        .unwrap();
                }
            }
            previous = (*path).to_owned();
        }
        builder.into_inner().unwrap()
    }

    pub fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    pub fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (path, content) in files {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn kinds_follow_the_extension() {
        assert_eq!(Kind::of("a.tar.gz"), Some(Kind::TarGz));
        assert_eq!(Kind::of("a.TAR.XZ"), Some(Kind::TarXz));
        assert_eq!(Kind::of("a.zip"), Some(Kind::Zip));
        assert_eq!(Kind::of("a.tar"), None);
    }

    #[test]
    fn only_the_soname_files_of_nvidia_are_kept() {
        let linux = nvidia_pick(Platform::Linux, 13, true);
        let top = "libcublas-linux-x86_64-13.7.0.27-archive";
        assert_eq!(
            linux(&format!("{top}/lib/libcublas.so.13.7.0.27")).as_deref(),
            Some("libcublas.so.13")
        );
        assert_eq!(
            linux(&format!("{top}/lib/libcublasLt.so.13.7.0.27")).as_deref(),
            Some("libcublasLt.so.13")
        );
        assert_eq!(
            linux(&format!("{top}/lib/libcudart.so.13")).as_deref(),
            Some("libcudart.so.13")
        );
        assert_eq!(
            linux(&format!("{top}/LICENSE")).as_deref(),
            Some(NVIDIA_LICENSE)
        );
        for refused in [
            "lib/libcublas.so.13.7",
            "x/lib/stubs/libcublas.so.13",
            "x/lib/libcublas_static.a",
            "x/lib/libcublas.so",
            "x/lib/libcublas.so.12.1",
            "x/lib/libcublas.so.13.x",
            "x/lib/libcublas.so.13.",
            "x/include/LICENSE",
            "x/bin/x64/cublas64_13.dll",
        ] {
            assert_eq!(linux(refused), None, "{refused}");
        }
        // Sans `license` : la licence n'est prise que d'une archive.
        assert_eq!(nvidia_pick(Platform::Linux, 13, false)("x/LICENSE"), None);
        let windows = nvidia_pick(Platform::Windows, 13, false);
        assert_eq!(
            windows("cuda_cudart-windows/bin/x64/cudart64_13.dll").as_deref(),
            Some("cudart64_13.dll")
        );
        assert_eq!(
            windows("libcublas-windows/bin/CUBLASLT64_13.DLL").as_deref(),
            Some("cublasLt64_13.dll")
        );
        assert_eq!(windows("x/lib/x64/cudart.lib"), None);
        assert_eq!(windows("x/bin/x64/cudart64_12.dll"), None);
        assert_eq!(windows("x/lib/libcudart.so.13"), None);
    }

    #[test]
    fn tar_xz_and_zip_archives_are_extracted_flat() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        // .tar.xz de NVIDIA : fichier versionné pris, lien et statique ignorés.
        let tar = tar_bytes(&[
            ("top/lib/libcudart.so.13.4.49", Some(b"cudart")),
            ("top/lib/libcudart.so.13", None),
            ("top/lib/libcudart_static.a", Some(b"static")),
            ("top/LICENSE", Some(b"eula")),
        ]);
        let mut xz = Vec::new();
        {
            let mut writer =
                lzma_rust2::XzWriter::new(&mut xz, lzma_rust2::XzOptions::with_preset(1)).unwrap();
            writer.write_all(&tar).unwrap();
            writer.finish().unwrap();
        }
        let archive = dir.path().join("cudart.tar.xz");
        std::fs::write(&archive, &xz).unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let written = extract(
            &archive,
            Kind::TarXz,
            &out,
            &nvidia_pick(Platform::Linux, 13, true),
            &cancel,
        )
        .unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(
            std::fs::read(out.join("libcudart.so.13")).unwrap(),
            b"cudart"
        );
        assert_eq!(std::fs::read(out.join(NVIDIA_LICENSE)).unwrap(), b"eula");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2);

        // .zip de Windows.
        let zip = zip_bytes(&[
            ("top/bin/x64/cublas64_13.dll", b"cublas"),
            ("top/bin/x64/cublasLt64_13.dll", b"lt"),
            ("top/lib/x64/cublas.lib", b"lib"),
        ]);
        let archive = dir.path().join("cublas.zip");
        std::fs::write(&archive, zip).unwrap();
        let out = dir.path().join("win");
        std::fs::create_dir(&out).unwrap();
        let written = extract(
            &archive,
            Kind::Zip,
            &out,
            &nvidia_pick(Platform::Windows, 13, false),
            &cancel,
        )
        .unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(std::fs::read(out.join("cublasLt64_13.dll")).unwrap(), b"lt");
    }

    #[test]
    fn a_duplicate_or_a_cancelled_extraction_fails() {
        let dir = tempfile::tempdir().unwrap();
        let tar = gz(&tar_bytes(&[
            ("VERSION", Some(b"a")),
            ("./VERSION", Some(b"b")),
        ]));
        let archive = dir.path().join("pack.tar.gz");
        std::fs::write(&archive, tar).unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let error = extract(
            &archive,
            Kind::TarGz,
            &out,
            &pack_pick(Platform::Linux),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate"), "{error}");
        let out = dir.path().join("cancelled");
        std::fs::create_dir(&out).unwrap();
        let error = extract(
            &archive,
            Kind::TarGz,
            &out,
            &pack_pick(Platform::Linux),
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        // Règle du pack : la racine seulement, noms connus.
        let pick = pack_pick(Platform::Windows);
        assert_eq!(pick("ggml-cuda.dll").as_deref(), Some("ggml-cuda.dll"));
        assert_eq!(pick("sub/ggml-cuda.dll"), None);
        assert_eq!(pick("libggml-cuda.so"), None);
        assert!(!safe_name("../x") && !safe_name("a\\b") && safe_name("VERSION"));
    }
}
